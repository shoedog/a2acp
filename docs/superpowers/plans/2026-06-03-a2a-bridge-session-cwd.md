# Session cwd / Per-Request Repo Targeting — Implementation Plan (rev2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Let the ACP `session/new` working directory differ from any host path and be set **per request** (via `a2a-bridge.cwd` metadata) — so one `serve` drives agents against many codebases (host or containerized) with no identical-path mount hack.

**Architecture (rev2 — dual-designed + dual-reviewed):** cwd is a **session *location*, not LLM config** — it is NOT folded into `EffectiveConfig`. A validated `SessionCwd` newtype (parse-don't-validate) carries the guarantee; the per-session stash becomes a `SessionSpec { config: EffectiveConfig, cwd: Option<SessionCwd> }` threaded through the **existing** `configure_session`→`ensure_session` mint timing (reuse the seam, separate the type). The per-request cwd is a **distinct** `RoutedCall.session_cwd` (NOT `AgentOverride`, whose fields are dropped for workflows). Workflows thread a `WorkflowRunContext { session_cwd }` (covers **both** streaming and detached paths). Resolution at mint: **request cwd → static `session_cwd` → `cwd` → `"."`**. The detached path persists `tasks.session_cwd` (own column, W3b migration pattern) and re-validates it on resume.

**Tech Stack:** Rust, the existing `AgentBackend`/registry/`TaskStore`/`WorkflowExecutor`, rusqlite, ACP §11A session semantics, A2A message metadata.

**Spec:** `docs/superpowers/specs/2026-06-03-a2a-bridge-session-cwd-design.md`.
**Branch:** `feat/session-cwd` off `main`.

**Provenance (rev2):** rev1 (ride-`EffectiveConfig`) was dual-reviewed (Codex executability + Claude architecture) AND compared against a firewalled independent codex design. Both the independent design and Claude's review independently said *separate cwd from `EffectiveConfig`* (different invariants: validation, persistence, immutability-after-mint). rev2 adopts that separation + the `SessionCwd` newtype + the reuse-key guard + the immutability guard, and folds every executability blocker (below). The independent design's `spawn_cwd` field was **declined** (the host child has no cwd today — `Supervised::spawn(cmd,args,None)` — so it's YAGNI; the concept is documented).

**Grounding facts (confirmed against the code by the reviews):**
- `AgentBackend::configure_session(&self, session, cfg: &EffectiveConfig)` (bridge-core/ports.rs:41) stashes per-session config into `session_cfg: HashMap<SessionId, EffectiveConfig>` (acp_backend.rs:306, stash at 1355); `ensure_session` (acp_backend.rs:851) reads the stash before mint (867), derives `(mode,model,effort)`, and mints `new_session_request(cwd)` (acp_backend.rs:890) — `cwd` today comes ONLY from static `AcpConfig.cwd`. `forget_session` drops the stash.
- **The host child has NO cwd:** `AcpBackend::spawn` calls `Supervised::spawn(cmd, args, None)` (acp_backend.rs:471); `entry.cwd` feeds ONLY `AcpConfig.cwd` via the spawn closures (main.rs:153, main.rs:430). So "host spawn cwd" does not exist — `cwd`/`session_cwd` both denote the ACP session cwd.
- `AgentEntry` is a **core** struct (bridge-core/domain.rs:40) — adding a field ripples to every `AgentEntry { … }` literal: domain.rs, route.rs, registry.rs, executor.rs, server.rs, workflow_producer.rs, `bin/a2a-bridge/tests/common/mod.rs`, `bin/a2a-bridge/tests/e2e_registry.rs`. Registry reuse compares `cmd/args/cwd/auth` (registry.rs:269) — the per-request `session_cwd` is NOT on `AgentEntry`, so it must NOT become a respawn key.
- Per-request overrides parse in `task_meta_from_params` (server.rs:~2285-2333) into `AgentOverride` → `RoutedCall.overrides`; **`AgentOverride.{model,effort,mode}` are dropped for workflows** (executor.rs:93 passes `None`). `effective_config(entry, overrides)` (domain.rs:83) layers overrides; LOCAL/fan-out dispatch uses it (server.rs:387,424); `run_node` builds `effective_config`+`configure_session`+prompt (executor.rs:84,93).
- **Streaming** workflows route via `spawn_workflow_producer` → `executor.run(...)` (server.rs:547,1067); **detached** via `spawn_detached_workflow` → `run_from(...)`; resume via `resume_working_tasks`.
- `BridgeError::InvalidRequest { field: &'static str }` (error.rs:26). `row_to_task` is **positional** — `get`/`list`/`working_tasks` SELECTs (sqlite.rs:378,399,516) + `create` INSERT must all add the column or a `row.get(N)` panics.
- W3b additive-column pattern: `migrate_tasks_columns` (sqlite.rs:127, pragma table_info-guarded ALTER); `TaskRecord` additive fields ripple to every literal. `run_from(graph,input,run_id,cancel,seed)`; `spawn_detached_workflow(srv,task,input,graph,run_id,token,seed)`; `resume_working_tasks(srv,cap)`.
- Tests: workflow_producer.rs under `-p bridge-a2a-inbound`; `-p a2a-bridge` has no `--lib`. Coverage floors: workspace 85, bridge-core 90, bridge-workflow 90 (after `cargo llvm-cov clean --workspace`). `cargo check --workspace` after EVERY task.

---

## File Structure
- **Create** `crates/bridge-core/src/session_cwd.rs` (or add to `domain.rs`) — `SessionCwd` newtype (parse-don't-validate) + `is_under`.
- **Modify** `crates/bridge-core/src/domain.rs` — `SessionSpec { config, cwd }`; `AgentEntry.session_cwd`.
- **Modify** `crates/bridge-core/src/ports.rs` — `configure_session(&self, &SessionId, &SessionSpec)` (param type change; name kept).
- **Modify** `crates/bridge-acp/src/acp_backend.rs` — stash `SessionSpec`; `ensure_session` mints `spec.cwd ?? static`; immutability guard.
- **Modify** `crates/bridge-api/src/backend.rs` + every test `FakeBackend` — `configure_session(&SessionSpec)` signature.
- **Modify** `bin/a2a-bridge/src/config.rs` — `AgentEntryToml.session_cwd` + global `allowed_cwd_root`.
- **Modify** `bin/a2a-bridge/src/main.rs` — extract a unit-tested `resolve_static_session_cwd` helper; `AcpConfig.cwd` from `session_cwd ?? cwd ?? "."`; document host-child-no-cwd.
- **Modify** `crates/bridge-a2a-inbound/src/server.rs` — parse + validate `a2a-bridge.cwd` → `RoutedCall.session_cwd: Option<SessionCwd>`; single-agent dispatch builds `SessionSpec`; thread `WorkflowRunContext` into streaming + detached + resume; persist; allowed_cwd_root wiring.
- **Modify** `crates/bridge-workflow/src/executor.rs` — `WorkflowRunContext`; `run_with_context`/`run_from_with_context`; `run_node` builds `SessionSpec{node config, ctx.cwd}`.
- **Modify** `crates/bridge-core/src/task_store.rs` + `crates/bridge-store/src/sqlite.rs` — `TaskRecord.session_cwd` + column + migration + all 3 SELECTs.
- **Create** `docs/adr/0014-session-cwd.md`.

---

## Phase A — `SessionCwd` newtype (validated value, the foundation)

### Task 1: `SessionCwd` parse-don't-validate newtype

**Files:** Create `crates/bridge-core/src/session_cwd.rs`; modify `crates/bridge-core/src/lib.rs` (module export).

- [ ] **Step 1: Failing tests.**
```rust
    #[test]
    fn session_cwd_parse_rules() {
        assert!(SessionCwd::parse("/abs/repo").is_ok());
        assert_eq!(SessionCwd::parse("/a/b/../c").unwrap().as_str(), "/a/c"); // lexical ..-collapse
        assert!(SessionCwd::parse("rel/path").is_err());     // not absolute
        assert!(SessionCwd::parse("").is_err());             // empty
        assert!(SessionCwd::parse("/a\0b").is_err());        // NUL
    }
    #[test]
    fn session_cwd_is_under() {
        let c = SessionCwd::parse("/work/repo").unwrap();
        assert!(c.is_under(&SessionCwd::parse("/work").unwrap()));
        assert!(!SessionCwd::parse("/work-evil").unwrap().is_under(&SessionCwd::parse("/work").unwrap())); // component-wise, not str prefix
        assert!(!SessionCwd::parse("/a/../../etc").unwrap().is_under(&SessionCwd::parse("/a").unwrap())); // collapsed before check
    }
```
- [ ] **Step 2: Run → fails.** `cargo test -p bridge-core session_cwd`
- [ ] **Step 3: Implement.** `pub struct SessionCwd(String);` with:
  - `pub fn parse(s: &str) -> Result<SessionCwd, BridgeError>`: reject empty; reject `s.contains('\0')`; reject non-absolute (`!std::path::Path::new(s).is_absolute()`); **lexically normalize** by folding `Component::CurDir`/`ParentDir`/`Normal` into a `Vec<Component>` (drop `.`; pop on `..` unless it would escape root → on a `..` past root return `Err(InvalidRequest{field:"a2a-bridge.cwd"})`); rebuild the absolute path string. Return `Ok(SessionCwd(normalized))`.
  - `pub fn as_str(&self) -> &str`; `impl std::fmt::Display`.
  - `pub fn is_under(&self, root: &SessionCwd) -> bool`: component-wise — `Path::new(&self.0).components().collect::<Vec<_>>().starts_with(&root_components)` (both already normalized, so no `..` and no symlink resolution; document this is lexical, not a sandbox).
  - Use `BridgeError::InvalidRequest { field: "a2a-bridge.cwd" }` (note: `field` is `&'static str`).
  - Export the module from `lib.rs`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-core`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace`.
- [ ] **Step 5: Commit** (NO trailer): `git commit -m "feat(core): SessionCwd newtype — parse-don't-validate (absolute, normalized, allowed-root component check)"`

---

## Phase B — Config: `session_cwd` + `allowed_cwd_root` + static resolution

### Task 2: `AgentEntry.session_cwd` (core-struct ripple) + `AcpConfig` static cwd + `allowed_cwd_root`

**Files:** `crates/bridge-core/src/domain.rs` (`AgentEntry`), `bin/a2a-bridge/src/config.rs`, `bin/a2a-bridge/src/main.rs`, + every `AgentEntry { … }` literal.

- [ ] **Step 1: Failing tests.** (a) config parse test: `session_cwd` + global `allowed_cwd_root` parse and are optional (mirror the W3b `store_resume_attempt_cap_parses_and_defaults` test). (b) a pure-helper unit test in `main.rs`:
```rust
    #[test]
    fn resolve_static_session_cwd_chain() {
        assert_eq!(resolve_static_session_cwd(Some("/s"), Some("/c")), "/s");  // session_cwd wins
        assert_eq!(resolve_static_session_cwd(None, Some("/c")), "/c");        // falls to cwd
        assert_eq!(resolve_static_session_cwd(None, None), ".");               // default
    }
```
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.**
  - `domain.rs`: add `pub session_cwd: Option<String>` to `AgentEntry`. **Grep `AgentEntry {` workspace-wide** and add `session_cwd: None` to EVERY literal (domain.rs, route.rs, registry.rs, executor.rs, server.rs, workflow_producer.rs, `bin/a2a-bridge/tests/common/mod.rs`, `bin/a2a-bridge/tests/e2e_registry.rs`). Registry reuse comparison (registry.rs:269) is UNCHANGED — `session_cwd` is a mint-time default, NOT a respawn key (a per-request cwd reuses the warm process).
  - `config.rs`: `AgentEntryToml.session_cwd: Option<String>` (`#[serde(default)]`) mapped into `AgentEntry`; a top-level `pub allowed_cwd_root: Option<String>` (`#[serde(default)]`) carried to `InboundServer` (builder, mirroring W3b's `resume_attempt_cap` wiring).
  - `main.rs`: a pure `fn resolve_static_session_cwd(session_cwd: Option<&str>, cwd: Option<&str>) -> String { session_cwd.or(cwd).unwrap_or(".").to_string() }`; in BOTH spawn closures (153, 430) set `AcpConfig.cwd = PathBuf::from(resolve_static_session_cwd(...))` (absolute resolution preserved — if the existing code absolutized `cwd`, keep that for the resolved value). Add a comment: *the host child has no cwd (`Supervised` gets `None`); `AcpConfig.cwd` is the ACP session cwd.*
- [ ] **Step 4: Run → green.** `cargo test -p a2a-bridge -p bridge-core`, clippy, `cargo check --workspace`.
- [ ] **Step 5: Commit** `git commit -m "feat(core,config,bin): AgentEntry.session_cwd + allowed_cwd_root; static session-cwd resolution (host child has no cwd)"`

---

## Phase C — `SessionSpec` stash separation (backend)

### Task 3: `SessionSpec` + `configure_session(&SessionSpec)` + ripple (behavior-preserving)

**Files:** `crates/bridge-core/src/domain.rs` (`SessionSpec`), `crates/bridge-core/src/ports.rs`, `crates/bridge-acp/src/acp_backend.rs`, `crates/bridge-api/src/backend.rs`, every test `FakeBackend`.

- [ ] **Step 1: Failing test.** An acp test that calls `configure_session(&session, &SessionSpec{ config: <eff>, cwd: None })` and asserts the existing mint still uses the static cwd (behavior preserved) — i.e. the signature compiles + the model/mode stash still works.
- [ ] **Step 2: Run → fails** (configure_session takes `&EffectiveConfig`).
- [ ] **Step 3: Implement.**
  - `domain.rs`: `pub struct SessionSpec { pub config: EffectiveConfig, pub cwd: Option<SessionCwd> }`.
  - `ports.rs`: change `AgentBackend::configure_session` param `cfg: &EffectiveConfig` → `spec: &SessionSpec` (keep the method name; default body still `Ok(())`).
  - `acp_backend.rs`: change `session_cfg` to `HashMap<SessionId, SessionSpec>`; `configure_session` stashes the `SessionSpec`; `ensure_session` reads `(mode,model,effort)` from `spec.config` (unchanged logic). cwd still from static this task (cwd-read is Task 4).
  - `bridge-api/backend.rs` + every test `FakeBackend`: update the `configure_session` impl signature to `&SessionSpec` (read `spec.config` where they read the old cfg). Update every CALL SITE to pass `SessionSpec { config: <eff>, cwd: None }` (inbound dispatch server.rs:387,424; executor.rs:93).
- [ ] **Step 4: Run → green.** `cargo test --workspace`, clippy, check (behavior-preserving — all existing tests pass).
- [ ] **Step 5: Commit** `git commit -m "refactor(core,acp,api): per-session stash is SessionSpec{config,cwd}; configure_session takes &SessionSpec (cwd separated from EffectiveConfig)"`

### Task 4: `ensure_session` mints `spec.cwd` (fallback static) + immutability guard

**Files:** `crates/bridge-acp/src/acp_backend.rs`.

- [ ] **Step 1: Failing tests.** (a) stash `SessionSpec.cwd = Some(SessionCwd::parse("/req")?)` → next `ensure_session` mints `session/new` with cwd `/req` (recorded `NewSessionRequest`); (b) no stash cwd → mint uses static `AcpConfig.cwd`; (c) a session already minted with cwd `/a`, then `configure_session` with cwd `/b` for the same `SessionId` → the next operation returns `BridgeError::InvalidStateTransition` (or `ConfigInvalid`), NOT a silent ignore.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** In `ensure_session`, derive the mint cwd: `let cwd = stashed.as_ref().and_then(|s| s.cwd.as_ref()).map(|c| c.as_str().to_string()).or_else(|| self.config.as_ref().map(|c| c.cwd.to_string_lossy().into_owned())).ok_or(BridgeError::AgentCrashed)?;` then `new_session_request(PathBuf::from(cwd))`. **Immutability guard:** before reusing an already-minted `agent_id`, if the stashed `SessionSpec.cwd` differs from the cwd the session was minted with (record the mint cwd in the session entry at mint time), return `Err(BridgeError::InvalidStateTransition)` — a session's cwd is fixed at `session/new` (ACP §11A); silently serving a stale cwd is the worst failure mode.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-acp`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(acp): session/new mint honors SessionSpec.cwd; reject post-mint cwd change (immutability guard)"`

---

## Phase D — Inbound per-request cwd (distinct carrier)

### Task 5: parse + validate `a2a-bridge.cwd` → `RoutedCall.session_cwd`

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing tests.** (a) `message/send` with `metadata."a2a-bridge.cwd" = "/abs/repo"` → `RoutedCall.session_cwd == Some(SessionCwd("/abs/repo"))`; (b) relative cwd → `unary_message` returns `InvalidRequest`; (c) with `allowed_cwd_root="/work"`, `cwd="/work/r"` accepted, `cwd="/other"` → `InvalidRequest`.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** Add `session_cwd: Option<SessionCwd>` to `RoutedCall` (a DISTINCT field — NOT `AgentOverride`, whose fields are dropped for workflows). In `task_meta_from_params`/the gate (server.rs:~2285,287): read `a2a-bridge.cwd`; if present, `SessionCwd::parse(v)?` (propagating `InvalidRequest`), then if `srv.allowed_cwd_root` is `Some(root)` enforce `cwd.is_under(&SessionCwd::parse(root)?)` else `InvalidRequest`; store into `RoutedCall.session_cwd`. Reject BEFORE minting a task/dispatch via `bridge_err_to_jsonrpc`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): parse + validate a2a-bridge.cwd into RoutedCall.session_cwd (distinct from AgentOverride)"`

### Task 6: single-agent dispatch applies the per-request cwd (+ warm-reuse test)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing tests.** (a) a recording fake backend captures the `SessionSpec` passed to `configure_session`; a sync `message/send` (Local) with `a2a-bridge.cwd="/req"` → `spec.cwd == Some("/req")`. (b) **warm-reuse:** two sequential sends with DIFFERENT cwd to the same agent → the registry spawns the backend process **once** (assert the spawn-count is 1; mirror an existing registry-reuse test) — per-request cwd must NOT respawn.
- [ ] **Step 2: Run → fails** (dispatch passes `cwd: None`).
- [ ] **Step 3: Implement.** Where Local/fan-out dispatch builds the `SessionSpec` for `configure_session` (the Task-3 call sites), set `cwd: routed.session_cwd.clone()`. No new seam.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): single-agent dispatch sets SessionSpec.cwd from the request; warm process reused across cwds"`

---

## Phase E — Workflow per-request cwd (context: streaming + detached)

### Task 7: `WorkflowRunContext` threaded through both workflow paths

**Files:** `crates/bridge-workflow/src/executor.rs`, `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing tests.** (a) executor: `run_from_with_context(graph,input,run_id,cancel,seed, WorkflowRunContext{ session_cwd: Some("/req") })` → every node's recorded `SessionSpec.cwd == Some("/req")`; `None` context → `cwd None`. (b) inbound: a **streaming** workflow (`message/stream`) with `a2a-bridge.cwd="/req"` → nodes dispatched with `/req` (the streaming path was a rev1 gap). (c) a **detached** workflow submit with `/req` → same.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.**
  - `executor.rs`: `pub struct WorkflowRunContext { pub session_cwd: Option<SessionCwd> }`. Add `run_with_context(graph,input,run_id,cancel,ctx)` and `run_from_with_context(graph,input,run_id,cancel,seed,ctx)`; keep `run`/`run_from` as wrappers passing `WorkflowRunContext::default()`. `run_node` takes the ctx (forwarded opaque — NOT read by `schedule_ready!`/topo) and builds `SessionSpec { config: <node effective_config>, cwd: ctx.session_cwd.clone() }` for `configure_session`.
  - `server.rs`: `spawn_workflow_producer` (STREAMING, server.rs:1067) calls `run_with_context(..., ctx)` with `ctx.session_cwd = routed.session_cwd`; `spawn_detached_workflow` gains a `ctx: WorkflowRunContext` param and calls `run_from_with_context`; the unary detached arm passes the request cwd; test seams pass `WorkflowRunContext::default()`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-workflow -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(workflow,inbound): WorkflowRunContext threads per-request cwd to every node (streaming + detached)"`

---

## Phase F — Persist + resume

### Task 8: `TaskRecord.session_cwd` + column + migration + persist at submit

**Files:** `crates/bridge-core/src/task_store.rs`, `crates/bridge-store/src/sqlite.rs`, `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing tests.** (a) `task_store.rs` Memory round-trip of `TaskRecord.session_cwd`. (b) `sqlite.rs`: round-trip + migration (old DB reopen → `session_cwd` NULL/`None`). (c) `server.rs`: detached submit with `a2a-bridge.cwd="/req"` persists `TaskRecord.session_cwd == Some("/req")`.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.**
  - `task_store.rs`: `pub session_cwd: Option<String>` on `TaskRecord`; **grep `TaskRecord {` and add `session_cwd: None`** to every literal (server.rs arm, workflow_producer.rs tests, sqlite.rs tests, task_store.rs tests).
  - `sqlite.rs`: `session_cwd TEXT` in `migrate_tasks_columns`; add to `create` INSERT; **add to ALL THREE SELECTs (`get` :378, `list` :399, `working_tasks` :516) + `row_to_task` positional mapping** (a `row.get(N)` against a short SELECT is a runtime `StoreFailure` — update them together).
  - `server.rs` detached arm: persist `session_cwd: routed.session_cwd.as_ref().map(|c| c.as_str().to_string())`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-core -p bridge-store -p bridge-a2a-inbound`, clippy, `cargo check --workspace`.
- [ ] **Step 5: Commit** `git commit -m "feat(core,store,inbound): persist TaskRecord.session_cwd (additive column + migration + all SELECTs)"`

### Task 9: resume re-validates + restores session_cwd (corrupt → Interrupted)

**Files:** `crates/bridge-a2a-inbound/src/server.rs` (`resume_working_tasks`).

- [ ] **Step 1: Failing tests.** (a) a `Working` task with `session_cwd="/req"` + snapshot + a `codex`-only checkpoint → `resume_working_tasks` re-runs nodes with `SessionSpec.cwd == Some("/req")` (recording fake; poll to terminal per the W3b pattern). (b) a `Working` task with a **corrupt** `session_cwd="relative"` (or escaping) → resume marks it `Interrupted` ("unreadable session cwd"), does NOT spawn.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** In `resume_working_tasks`, after loading `wt.session_cwd`: if `Some(s)`, `SessionCwd::parse(s)` — on `Err` → `set_terminal(Interrupted, "unreadable session cwd")` + continue (re-validate, never trust the stored string blindly); on `Ok(c)` build `WorkflowRunContext{ session_cwd: Some(c) }`; `None` → default context. Pass the ctx into `spawn_detached_workflow(... ctx)` → `run_from_with_context`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): boot resume re-validates + restores session_cwd; corrupt cwd -> Interrupted"`

---

## Phase G — Verification, live gate, ADR

### Task 10: full sweep + coverage
- [ ] `cargo fmt --all && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` → all green.
- [ ] `cargo llvm-cov clean --workspace` then `--workspace --fail-under-lines 85`, `--package bridge-core --fail-under-lines 90`, `--package bridge-workflow --fail-under-lines 90`. Top up tests if any floor dips; commit additions.

### Task 11: gated live (DoD-6 — distinct cwd ≠ session_cwd; per-request subdir)
- [ ] Reuse the `a2a-claude-agent` image. Config a containerized agent with **`session_cwd="/work"` (in-container) ≠ host paths**, `-v <repo>:/work -w /work` (NO identical-path hack). Submit a `dev-task`; verify the container edits the repo + `Completed` — proving session_cwd works without identical paths. Then a **broad parent mount** (`-v /Users/wesleyjinks/code:/Users/wesleyjinks/code`) + a sync send with `a2a-bridge.cwd="/Users/wesleyjinks/code/<subdir>"` (ABSOLUTE — not `~`); verify the agent operated in that subdir. Record both in the ADR.

### Task 12: ADR-0014
- [ ] Write `docs/adr/0014-session-cwd.md`: the decision (cwd is a session location, NOT `EffectiveConfig` — `SessionCwd` newtype + `SessionSpec` stash riding the existing mint timing; distinct `RoutedCall.session_cwd`; `WorkflowRunContext` for both workflow paths; persist+re-validate-on-resume); the **dual-design + dual-review provenance** (the firewalled independent design + Claude review both drove the separation; `spawn_cwd` declined as YAGNI; the reuse-key + immutability guards); the live-gate result. Commit with the controller trailer.

---

## DoD (spec §Definition of Done) → tasks
| DoD | Task |
|-----|------|
| 1 (session_cwd config; session/new uses it; back-compat) | 2, 4 |
| 2 (a2a-bridge.cwd extracted + validated; absolute; allowed_cwd_root) | 1, 5 |
| 3 (single-agent dispatch honors per-request cwd) | 6 |
| 4 (workflow dispatch applies cwd to all nodes — streaming + detached; executor pure) | 7 |
| 5 (tasks.session_cwd persisted + restored on resume; migration) | 8, 9 |
| 6 (identical-path hack no longer required — distinct cwd/session_cwd) | 11 (live) |
| 7 (fmt/clippy/coverage; ADR) | 10, 12 |

## Notes for the implementer
- **cwd is a session *location*, NOT `EffectiveConfig`.** The stash type is `SessionSpec { config, cwd }`; it rides the existing `configure_session`→`ensure_session` mint timing (reuse the seam, separate the type). This is the rev2 architecture decision (dual-design + dual-review converged on it).
- **`SessionCwd` is parse-don't-validate** — the only way to get one is through `parse` (absolute, normalized, NUL-free), so the mint + resume + gate all receive a *guaranteed-valid* value. The `allowed_cwd_root` policy check is applied at the inbound gate (Task 5), not in the newtype.
- **Per-request cwd is `RoutedCall.session_cwd`, NOT `AgentOverride`** — `AgentOverride` is dropped for workflows; a distinct field keeps single-agent + workflow uniform.
- **Both workflow paths:** streaming (`spawn_workflow_producer`→`run`) AND detached (`spawn_detached_workflow`→`run_from`) must thread `WorkflowRunContext` — the streaming path was a rev1 miss.
- **The host child has NO cwd** (`Supervised::spawn(...,None)`); `session_cwd`/`cwd` denote the ACP session cwd. No `spawn_cwd` field (YAGNI). `session_cwd` is NOT a registry respawn key (Task 6 asserts warm reuse across cwds).
- **`AgentEntry` is core** → the `session_cwd` field ripples to every `AgentEntry {` literal (Task 2); `row_to_task` is positional → all 3 SELECTs + INSERT (Task 8); `InvalidRequest.field` is `&'static str`.
- **Executor purity:** `WorkflowRunContext` is forwarded into `run_node`, never read by scheduling/topo.
- **`run cargo check --workspace` after every task.** Firewall: design from bridge ports + ACP §11A + A2A metadata; a2a-local-bridge did not inform it. Controller docs (this plan, ADR-0014) carry the `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer; task commits do NOT. Coverage after `cargo llvm-cov clean --workspace`.
