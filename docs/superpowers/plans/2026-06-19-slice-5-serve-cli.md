# Slice 5 — Serve-backed `run-workflow` + handle-aware keep-warm — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (or executing-plans).
> Steps use checkbox (`- [ ]`).

**Goal:** `run-workflow --serve --context C <wf>` makes the CLI a serve client so a workflow's per-node agent
sessions stay warm in the serve and reuse across invocations (no cold respawn on the 2nd run); handle-aware
executor keep-warm (no per-node `forget_session`); drain-on-cancel preserved; non-serve path byte-identical.

**Architecture:** A dependency-inversion `WorkflowNodeDispatcher` (cold INLINE in `bridge-workflow`; warm impl
in `bridge-a2a-inbound`) keyed by derived per-node child contexts `<C>::workflow::<wf>::node::<node>`.
`SessionManager` owns child lifecycle atomically. A parent-context workflow-run guard handles concurrency +
scheduler-cancel.

**Spec:** `docs/superpowers/specs/2026-06-19-slice-5-serve-cli.md` v2 — **FIX-1..11 are BINDING**; this plan
implements them. Read the FIX list before each task.

**Tech stack:** Rust (tokio, async-trait, axum/reqwest); crates `bridge-workflow`, `bridge-a2a-inbound`,
`bridge-core`, `bin/a2a-bridge`. Each task ends green under `cargo test --workspace --no-run` + its own tests.

## v2 — dual plan-review fixes folded (BINDING; supersede any contradicting task snippet below)

Both reviewers returned `fix-then-execute`; layering keystone confirmed sound. These PFIXes are binding:

- **PFIX-1 (BLOCKER, the keystone — follow the SPEC's parameter-threaded seam, NOT a field/builder).** The
  server holds `Arc<WorkflowExecutor>` (`server.rs:149/1695`) so `with_node_dispatch(self)` can't move out of
  the Arc, AND a `node_dispatch` FIELD breaks the internal rebuild `WorkflowExecutor{registry:...}` at
  `executor.rs:222`. Instead (spec §2): add `run_with_context_and_dispatcher(&self, ..., dispatcher: Arc<dyn
  WorkflowNodeDispatcher>)` + `run_from_with_context_and_dispatcher(...)` that THREAD `dispatcher` as a
  parameter down to `run_node` (which takes `dispatcher: Option<&Arc<dyn WorkflowNodeDispatcher>>`; the existing
  cold methods pass `None`; inside the `:222` rebuild, pass the dispatcher into the per-node closure, do NOT
  add a struct field). T6 calls `executor.run_with_context_and_dispatcher(..., Arc::new(WarmWorkflowNode
  Dispatcher{..}))` on the `&WorkflowExecutor` (Arc derefs). DELETE the `node_dispatch` field + `with_node_dispatch`.
- **PFIX-2 (BLOCKER — `run_node` returns `(String,bool)`, not `Result`).** The warm branch must `match
  dispatcher.checkout(...).await { Ok(t)=>.., Err(e)=> return (format!("[node {} failed: {:?}]", node.id, e),
  false) }` — NOT `?` (mirror the cold marker `executor.rs:98/100`).
- **PFIX-3 (BLOCKER — warm cancel must NOT double-cancel).** The cold drain loop calls `backend.cancel` at
  `:137`. In the WARM path the loop's cancel arm must NOT call `backend.cancel` (the cleanup owns cancel via
  `sm.cancel(child)`, FIX-7) — else `backend.cancel` fires twice and the claim-defer/ABA invariant is bypassed.
  Keep the COLD loop fully inline + byte-identical (the `cancel_drains_inflight` keystone test, `executor.rs:
  ~870`, must stay green); give the WARM path its own loop body or an `is_warm` flag gating the `:137` arm.
- **PFIX-4 (BLOCKER — `checkout_child_turn` atomicity).** Register `parent→child` ATOMICALLY with the checkout:
  hold the `children` mutex ACROSS `checkout_turn(child,...)` + the insert (register on success); the
  `*_with_children` sweeps take the `children` mutex FIRST so a sweep waits for an in-progress child checkout.
  Consistent lock order children→`by_context` (no deadlock). (Register-after-release leaves a leak window.)
- **PFIX-5 (BLOCKER — cancel/release/clear ALWAYS sweep children, FIX-11).** Not only during an active run:
  `session_cancel C` = (if `workflow_runs[C]` token → `token.cancel()`) THEN ALWAYS `sm.cancel_with_children(C)`;
  `session_release/clear C` → `release/clear_with_children(C)`. All tolerant of an absent parent handle
  (success, not `SessionNotFound`). Add `session_cancel_workflow_parent_sweeps_children` (post-run).
- **PFIX-6 (MAJOR — HandleBusy placement).** The concurrent-run `HandleBusy` early-reject lives in
  `stream_message`'s `RouteTarget::Workflow` arm (`:823`) BEFORE building the SSE response (`:829`) — NOT in
  `spawn_workflow_producer` (fire-and-forget, returns `()`). The producer's signature gains `C`+token+dispatcher
  so it removes `workflow_runs[C]` on exit (mirror `workflow_cancels` :1711/:1731; note `routed.task`/`session_cwd`
  move order :1690/:1716).
- **PFIX-7 (MAJOR — the CLI SSE client).** The `--serve` client POSTs `SendStreamingMessage` (NOT `rpc_call`,
  which `.json()`s a unary body); drains the SSE byte-stream (the `task_watch_cmd` loop is INLINE `:2756` —
  extract/duplicate it); JSON-parses each `data:` frame as `a2a::StreamResponse` to find the terminal
  `StatusUpdate{state∈{Completed,Failed,Canceled}, message:None}` (exit code) + collect `ArtifactUpdate` text
  (stdout/`--out`). "Reuse the parser" is not literal — there is no parser fn.
- **PFIX-8 (MAJOR — CLI parser).** Parse flags ANYWHERE + require exactly one non-flag = the workflow id
  (`--serve --context C <wf>` has flags before the positional). Update `parse_run_workflow_args`' return tuple
  AND the 2 existing tests that destructure it (`main.rs:~4567/~4579`). The `--serve` branch early-returns
  BEFORE the local setup (snapshot/lease/spawn/registry/executor, `:2282-2358`).
- **PFIX-9 (MINOR — test harness).** T6/T7 streaming tests use the `with_workflows` integration harness (not
  just `seed_test_server`) + a GATED/blocking node backend (hold run #1 in `workflow_runs[C]` while #2 arrives).
- **PFIX-10 (MINOR — seed test).** The executor test prompt-recorder records concatenated strings; extend it to
  keep `Vec<Part>` boundaries so `warm_seed_prepended` can assert the FIRST part.
- **PFIX-11 (MINOR — clear-on-parent).** `session_clear` maps `NotFound`→`SessionNotFound` error (`:3037`);
  for a workflow parent (never a handle), a children-only `clear_with_children` must return SUCCESS — change the
  handler, not just the SM helper.

---

## File Structure
- `crates/bridge-workflow/Cargo.toml` — `async-trait` dev→`[dependencies]` (FIX-1).
- `crates/bridge-workflow/src/executor.rs` — `WorkflowNodeDispatcher` trait + `NodeTurn`/`NodeTurnExit`/
  `NodeTurnCleanup`; `WorkflowExecutor.node_dispatch: Option<Arc<dyn WorkflowNodeDispatcher>>`; the
  `*_and_dispatcher` entry methods; `run_node` warm branch (cold inline UNCHANGED) + seed prepend.
- `crates/bridge-a2a-inbound/src/session_manager.rs` — `children` map + `checkout_child_turn` +
  `{release,clear,cancel}_with_children` + `expire_turn` (FIX-2).
- `crates/bridge-a2a-inbound/src/server.rs` — `WarmWorkflowNodeDispatcher` (FIX-2/5/6/7); the workflow-run
  guard + `spawn_workflow_producer` warm wiring + `SessionCancel` scheduler-cancel (FIX-3); gate lift
  streaming-only + unary reject + the sweep wire handlers (FIX-8/10/11).
- `bin/a2a-bridge/src/main.rs` — `parse_run_workflow_args` (`--serve`/`--url`/`--context`; `--config`+`--serve`
  reject) + `run_workflow_cmd` serve-client branch (FIX-11).

---

### Task 1: `WorkflowNodeDispatcher` trait + types + async-trait dep (FIX-1, FIX-5)

**Files:** `crates/bridge-workflow/Cargo.toml`, `crates/bridge-workflow/src/executor.rs`

- [ ] **Step 1: Move `async-trait` to `[dependencies]`** in `crates/bridge-workflow/Cargo.toml` (it's currently
  `[dev-dependencies]`, `:16`): add `async-trait = { workspace = true }` to `[dependencies]` (keep or drop the
  dev entry).
- [ ] **Step 2: Add the trait + types** (executor.rs, near `WorkflowRunContext`):
```rust
pub enum NodeTurnExit { Normal, Canceled, Error(BridgeError) }

#[async_trait::async_trait]
pub trait NodeTurnCleanup: Send {
    /// Invoked once after prompt+drain on the node's exit branch. Each impl closes over what it owns
    /// (cold: backend+session for forget; warm: SessionManager+child+gen+op for finish/cancel/expire).
    async fn on_exit(self: Box<Self>, exit: NodeTurnExit);
}

pub struct NodeTurn {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
    pub seed: Option<String>,               // warm-only; prepended to the node prompt parts (FIX/Slice-4)
    pub cleanup: Box<dyn NodeTurnCleanup>,
}

#[async_trait::async_trait]
pub trait WorkflowNodeDispatcher: Send + Sync {
    async fn checkout(
        &self, wf_id: &str, node: &WorkflowNode, run_id: &str, ctx: &WorkflowRunContext,
    ) -> Result<NodeTurn, BridgeError>;
}
```
  (`AgentBackend` is `bridge_core::ports::AgentBackend` — import it.)
- [ ] **Step 3: Write a compile/behaviour test** — a `#[cfg(test)]` `FakeDispatcher` returning a `NodeTurn` with
  a counting `NodeTurnCleanup`; assert `on_exit(Normal)` runs. Run `cargo test -p bridge-workflow --lib` (+
  `--no-run` workspace). Commit.

---

### Task 2: Executor warm branch (cold INLINE unchanged) + seed prepend + drain preserved (FIX-4)

**Files:** `crates/bridge-workflow/src/executor.rs`

- [ ] **Step 1:** Add `node_dispatch: Option<Arc<dyn WorkflowNodeDispatcher>>` to `WorkflowExecutor`; `new`
  leaves it `None`; add `with_node_dispatch(self, d) -> Self`. Add
  `run_from_with_context_and_dispatcher(...)`/`run_with_context_and_dispatcher(...)` that set the dispatch then
  delegate to the existing run (so `spawn_workflow_producer` can opt in). The EXISTING `run_*` keep
  `node_dispatch=None`.
- [ ] **Step 2 (tests):** (a) `cold_path_unchanged` — existing executor tests still green (the `None` branch is
  byte-identical: same `workflow-{wf}-{node}-{run_id}` id, forget at every site). (b) `warm_dispatch_no_forget`
  — a `FakeDispatcher` (None-forget) + a recording backend → assert NO `forget_session`, the node prompted on
  the dispatcher's session, `cleanup.on_exit(Normal)` ran. (c) `warm_seed_prepended` — `NodeTurn.seed=Some("S")`
  → the prompt's FIRST part is the wrapped seed. (d) `dispatcher_cancel_drains` — cancel mid-run →
  `on_exit(Canceled)` + the `FuturesUnordered` drain still completes (W3b).
- [ ] **Step 3 (impl):** in `run_node`, branch at the top: if `self.node_dispatch.is_some()` → WARM path:
  `let turn = dispatch.checkout(wf_id, node, run_id, ctx).await?` (on Err, return the node-error marker — do
  NOT panic); build `parts` = `seed`-prepended (if `turn.seed`, `parts.insert(0, Part{text: format!("[Summary
  of earlier context in this session]\n{seed}")})`) then the rendered prompt; run the SHARED prompt+drain loop
  on `turn.backend`/`turn.session`; on each exit branch call `turn.cleanup.on_exit(exit)` with the right
  `NodeTurnExit` (Normal / Canceled / Error(e)) — REPLACING the cold `forget_session` calls. ELSE → the
  EXISTING inline cold path UNCHANGED. Keep the `FuturesUnordered` scheduler (`:322`) untouched.
  (Extract the prompt+drain loop into a shared helper if cleaner, but do NOT change the cold path's observable
  behavior.)
- [ ] **Step 4:** `cargo test -p bridge-workflow --lib && cargo test --workspace --no-run`. Commit.

---

### Task 3: SessionManager `children` map + atomic `checkout_child_turn` + `expire_turn` (FIX-2)

**Files:** `crates/bridge-a2a-inbound/src/session_manager.rs`

- [ ] **Step 1 (tests):** `checkout_child_turn_registers_and_reuses` — first call mints child + registers
  `parent→child`; second call (after finish) REUSES (same backend_session); `WarmTurn` carries the exact
  `generation`+`op`; `checkout_child_turn_failure_does_not_register` (a failing resolve/configure leaves no
  child entry).
- [ ] **Step 2 (impl):** add `children: Mutex<HashMap<ContextId, HashSet<ContextId>>>` to `SessionManager`
  (init empty in both constructors). Add:
```rust
pub async fn checkout_child_turn(
    &self, parent: &ContextId, child: &ContextId, agent: AgentId,
    overrides: Option<AgentOverride>, cwd: Option<SessionCwd>, op: OperationId,
) -> Result<WarmTurn, BridgeError> {
    let turn = self.checkout_turn(child, agent, overrides, cwd, op).await?; // existing warm reuse/mint
    self.children.lock().await.entry(parent.clone()).or_default().insert(child.clone()); // register on SUCCESS
    Ok(turn)
}
pub async fn expire_turn(&self, ctx: &ContextId) { self.release(ctx).await; } // backend process gone (FIX-6)
```
  (Registration AFTER `checkout_turn` success → no stale entry on failure, FIX-2/M3.)
- [ ] **Step 3:** `cargo test -p bridge-a2a-inbound --lib checkout_child && cargo test --workspace --no-run`. Commit.

---

### Task 4: SessionManager `{release,clear,cancel}_with_children` sweep (FIX-2)

**Files:** `crates/bridge-a2a-inbound/src/session_manager.rs`

- [ ] **Step 1 (tests):** `release_with_children_sweeps` — register 2 children under C; `release_with_children(C)`
  releases C (if present) + both children + clears the map entry; tolerant of an absent parent handle
  (success). Same shape for `cancel_with_children` (cancel each child + C) and `clear_with_children` (reset each).
- [ ] **Step 2 (impl):** add the three helpers — each snapshots `children[C]` under the lock, then for the
  parent + each child calls the existing op (`release`/`cancel`/`reset_session`) tolerant of absent handles;
  `release_with_children` also removes the `children[C]` entry. (`SessionNotFound` on the absent parent is
  swallowed → success if any child swept, FIX-11.)
- [ ] **Step 3:** `cargo test -p bridge-a2a-inbound --lib with_children && cargo test --workspace --no-run`. Commit.

---

### Task 5: `WarmWorkflowNodeDispatcher` in bridge-a2a-inbound (FIX-2/5/6/7)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`

- [ ] **Step 1 (tests):** `warm_workflow_dispatch_checks_out_child` — derives the child ctx
  `<C>::workflow::<wf>::node::<node>`, `checkout_child_turn`, returns `NodeTurn{seed}`; `on_exit(Normal)` →
  `finish_turn` (NOT forget); `on_exit(Canceled)` → `sm.cancel(child)`; `on_exit(Error(AgentCrashed))` →
  `sm.expire_turn(child)`; `on_exit(Error(other))` → `finish_turn`.
- [ ] **Step 2 (impl):** add `struct WarmWorkflowNodeDispatcher { sm: Arc<SessionManager>, parent: ContextId,
  cwd: Option<SessionCwd> }` implementing `WorkflowNodeDispatcher::checkout`: derive `child =
  ContextId::parse(format!("{}::workflow::{}::node::{}", parent, wf_id, node.id))`; mint an `op` from
  `(run_id, node.id)`; `let turn = sm.checkout_child_turn(parent, &child, node.agent.clone(), None, cwd, op)`;
  return `NodeTurn{ backend: turn.backend, session: turn.session, seed: turn.seed, cleanup: Box::new(WarmNode
  Cleanup{ sm, child, gen: turn.generation, op: turn.op }) }`. The `WarmNodeCleanup::on_exit` matches
  `NodeTurnExit` per FIX-6/7 (Normal/Error(other)→`finish_turn(child,gen,&op)`; Canceled→`sm.cancel(child)`;
  Error(AgentCrashed)→`sm.expire_turn(child)`).
- [ ] **Step 3:** `cargo test -p bridge-a2a-inbound --lib warm_workflow && cargo test --workspace --no-run`. Commit.

---

### Task 6: Workflow-run guard + spawn_workflow_producer warm wiring + SessionCancel scheduler-cancel (FIX-3)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`

- [ ] **Step 1 (tests):** `concurrent_same_context_workflow_handle_busy` — two streaming workflow sends on C →
  the 2nd returns `HandleBusy` (JSON-RPC) early; `session_cancel_cancels_workflow_run` — `SessionCancel C`
  cancels the parent run token (the executor stops scheduling) + `cancel_with_children(C)`.
- [ ] **Step 2 (impl):** add `workflow_runs: Mutex<HashMap<ContextId, CancellationToken>>` to `InboundServer`.
  In the `RouteTarget::Workflow` STREAMING dispatch (`stream_message` → `spawn_workflow_producer`), when
  `routed.context_id` is `Some(C)`: BEFORE returning the SSE response, lock `workflow_runs`; if `C` present →
  `HandleBusy`; else insert `C→token`. Build the executor with
  `.with_node_dispatch(Arc::new(WarmWorkflowNodeDispatcher{sm, parent:C, cwd}))` + pass the token as the run's
  cancel. On producer exit, remove `workflow_runs[C]` (mirror `workflow_cancels` :1711/:1731). In
  `session_cancel` (`server.rs:3104`): if `workflow_runs[C]` exists, `token.cancel()` (stop the scheduler) THEN
  `sm.cancel_with_children(C)`; else fall back to `sm.cancel(C)`.
- [ ] **Step 3:** `cargo test -p bridge-a2a-inbound --lib 'workflow_run|session_cancel_cancels' && cargo test
  --workspace --no-run`. Commit.

---

### Task 7: gate lift (streaming-only) + unary reject + sweep wire handlers (FIX-8/10/11)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`

- [ ] **Step 1 (tests):** `gate_allows_contextid_on_workflow_streaming`; `gate_rejects_unary_workflow_contextid`;
  `gate_still_rejects_delegate_fanout_contextid`; `session_release_workflow_parent_sweeps_children` (release C
  on a workflow parent succeeds + frees children).
- [ ] **Step 2 (impl):** in `gate()` (`:352`) change the rejection to `if context_id.is_some() &&
  !matches!(target, RouteTarget::Local(_) | RouteTarget::Workflow(_))` (FIX-10). In the UNARY `SendMessage`
  path, reject `routed.context_id.is_some() && matches!(target, Workflow)` (unary workflow = detached, deferred
  — FIX-8). Change the `SessionRelease`/`SessionClear` handlers (`server.rs:3005/3030`) to call
  `sm.release_with_children`/`clear_with_children` and treat an absent parent handle as success (FIX-11). (The
  `SessionCancel` sweep was done in T6.)
- [ ] **Step 3:** `cargo test -p bridge-a2a-inbound --lib 'gate_|session_release_workflow' && cargo test
  --workspace --no-run`. Commit.

---

### Task 8: CLI serve-client (FIX-11)

**Files:** `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1 (tests):** `run_workflow_context_requires_serve` (`--context` without `--serve` → error);
  `run_workflow_config_rejected_with_serve` (`--config` + `--serve` → error, detected on the un-defaulted
  Option); `serve_client_builds_streaming_message` (the message map has `contextId`, `metadata
  ["a2a-bridge.skill"]=<wf>`, parts).
- [ ] **Step 2 (impl):** extend `parse_run_workflow_args` (`:543`) with `--serve` (bool), `--url`
  (default `http://127.0.0.1:8080`), `--context` (Option); reject `--context` without `--serve`; reject explicit
  `--config` with `--serve` (check the raw Option BEFORE the `CONFIG_PATH` default). In `run_workflow_cmd`
  (`:2233`): if `--serve`, build a `SendStreamingMessage` message map (`message.contextId=C`,
  `metadata["a2a-bridge.skill"]=<wf>`, parts=input, cwd from `--session-cwd`) — build its OWN map, NOT via
  `submit_cmd` — POST to `--url`, consume the SSE stream (reuse the `task_watch_cmd` SSE PARSER `:2756`),
  printing artifact text to stdout/`--out` + statuses to stderr, exit code from the terminal state. ELSE the
  existing local one-shot path (UNCHANGED).
- [ ] **Step 3:** `cargo test -p a2a-bridge run_workflow && cargo test --workspace --no-run`. Commit.

---

### Task 9: Gate + live-gate + whole-branch review + merge

- [ ] **Step 1: Full gate** — `cargo test --workspace --no-run` → `cargo fmt --all --check` → `cargo clippy
  --workspace --all-targets -- -D warnings` → `cargo test --workspace > /tmp/s5-test.out 2>&1; echo $?` (REAL
  exit code, not piped to tail).
- [ ] **Step 2: Live-gate** (per spec §10) — `cargo build --release --bin a2a-bridge`; `serve --config
  examples/a2a-bridge.multi-agent.toml` (a multi-node workflow, large TTL); run `run-workflow --serve --url U
  --context C <wf> --input <f>` TWICE → prove the 2nd reuses warm agents (child checkouts HIT, same agent pids,
  no 2nd spawn, sub-second); `session release C` frees them; the non-serve `run-workflow <wf> --input <f>` still
  works.
- [ ] **Step 3: Whole-branch codex-xhigh review** (`git diff main...HEAD`) — the high-value cross-task pass;
  iterate-to-clean; fold blockers. Then FF-merge to `main` + push (operator authorizes); update HANDOFF +
  memory (Slices 0–5 ✅, **MVP COMPLETE**; NEXT = S6 journal).

---

## Self-review
- **Spec coverage:** FIX-1 (T1), FIX-2 (T3/T4/T5), FIX-3 (T6), FIX-4 (T2), FIX-5 (T1/T2/T5), FIX-6/7 (T5),
  FIX-8/10/11 (T7/T8), FIX-9 (scope, no code). Every FIX maps. ✓
- **Back-compat:** the cold executor path + local `run-workflow` are the `None`/no-`--serve` branches, untouched
  (T2/T8). Existing executor + run-workflow tests lock them.
- **Type consistency:** `WorkflowNodeDispatcher`/`NodeTurn`/`NodeTurnExit`/`NodeTurnCleanup` (T1) used in T2/T5;
  `checkout_child_turn`/`*_with_children`/`expire_turn` (T3/T4) used in T5/T6/T7. ✓
