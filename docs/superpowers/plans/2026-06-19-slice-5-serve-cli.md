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
  dispatcher.checkout(...).await { Ok(t)=>.., Err(e)=> return (format!("[node {} failed: {:?}]", node.id.as_str(), e),
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

### v3 — round-2 plan-review folds (codex xhigh; the PFIX intent folded INTO the task bodies)
Round-2 confirmed the seam is sound but flagged stale/non-compiling TASK-BODY snippets. Folded directly into the
tasks so a literal implementor is correct without cross-referencing the PFIX list:
- T2/T6/File-Structure: parameter-threaded dispatcher everywhere (no `node_dispatch` field); warm branch `match`
  on `d.checkout` (not `?`); warm cancel does NOT double-`backend.cancel`; KEEP the `:76` cancel pre-check before
  the warm branch (don't mint a child for an already-canceled run).
- T3/T4 (BLOCKER): `checkout_child_turn` HOLDS `children` across `checkout_turn`+insert; `*_with_children` sweeps
  take `children` FIRST and hold across the sweep (order children→by_context, no deadlock).
- T5 (BLOCKER, compile): derive the child id with `parent.as_str()`/`wf_id`/`node.id.as_str()` (ids have no
  `Display`).
- T6 (BLOCKER): `session_cancel` ALWAYS `cancel_with_children(C)` (no bare-`cancel` fallback); register the run
  token in BOTH `workflow_runs[C]` (SessionCancel) AND `workflow_cancels[task]` (CancelTask). New post-run test.
- T7 (MAJOR): place the unary workflow+context reject AFTER `gate()` but BEFORE `store.put` (`:2371`) — no state
  mutation on a rejected request.

### v4 — round-3 plan-review folds (codex xhigh; child-map LIFECYCLE + back-compat)
Round-3 confirmed the seam/ordering again, and caught deeper semantic bugs (not snippets). Folded into T4/T6/T7/T8:
- T4 (BLOCKER): **only `release_with_children` removes `children[C]`** — `cancel` idles & `reset_session` keeps
  the handle WARM, so cancel/clear must RETAIN the mapping (else a later `release C` strands the warm children).
  Added `cancel_then_release_frees_children` / `clear_then_release_frees_children`.
- T4/T7 (BLOCKER): **no blanket "zero children = success"** — `clear_with_children`/`cancel_with_children` return
  `NotFound`/`Err(SessionNotFound)` for an unknown ctx with no handle AND no children, preserving the existing
  unknown-clear 400 test (`server.rs:6955`). `SessionCancel` is success when an active run token existed.
- T4/T7 (MAJOR): **`clear_with_children(ctx, force: bool)`** threads `force` (ResetOpts is neither Copy nor
  Clone → build `ResetOpts { force }` per call); handler passes the parsed `force`.
- T8 (MAJOR): the `--serve` branch **early-returns before the local config read (`:2242`)** + any workflow
  lookup — `--config` must not be required in serve mode.

### v5 — round-4 plan-review folds (codex xhigh; producer-owned guard lifetime + clear strictness)
Round-4 re-confirmed seam/ordering and caught a guard-lifetime regression I introduced in v3:
- T6 (BLOCKER): `session_cancel` **GETs+clones the run token (does NOT remove it)** — the guard is released ONLY
  on producer exit. Removing it on cancel lets a 2nd same-context run overlap the still-draining first run and
  re-claim a child that `cancel` marked Idle (`:454`) but is still tearing down (`backend.cancel` `:461`). New
  test `session_cancel_keeps_context_busy_until_producer_exits`.
- T4 (MAJOR): `clear_with_children` **propagates real child errors** (`HandleBusy` on a Running child without
  `force`) instead of `let _ =`-swallowing them; `Ok(_)` still accepts `Cleared`/stale-`NotFound`. New test
  `clear_with_children_running_child_without_force_is_busy`.
- T5 (MINOR): derive the `op` as `OperationId::parse(format!("workflow-{run_id}-node-{}", node.id.as_str()))?`
  (`NodeId` has no `Display`).

### v6 — round-5 plan-review folds (codex xhigh; idempotent cancel + guard-before-store + cwd clone)
Round-5 re-confirmed seam/ordering/T8-early-return and caught 3 narrower issues:
- T4 (BLOCKER): make `SessionManager::cancel` idempotent — skip `backend.cancel` when `!was_running` (return
  `Ok(())`) so `cancel_with_children` + the executor's `on_exit(Canceled)→sm.cancel(child)` produce EXACTLY ONE
  `backend.cancel` per child (existing cancel tests cancel a Running handle → green). New unit test
  `cancel_idle_handle_skips_backend_cancel` + integration `active_session_cancel_one_backend_cancel_per_child`.
- T6 (MAJOR): place the same-context workflow `HandleBusy` guard RIGHT AFTER `gate()` and BEFORE the `:769`
  `store.put` (not in the `:823` Workflow arm) — a rejected 2nd workflow must not mutate `SessionStore`; thread
  the token into `spawn_workflow_producer`.
- T5 (MAJOR, compile): `checkout` takes `&self` → use `&self.parent`/`self.cwd.clone()`/`self.sm` (can't move
  `self.cwd` out of `&self`; `checkout_child_turn` takes an owned `Option<SessionCwd>`).

### v7 — round-6 plan-review folds (codex xhigh; warm/cold back-compat + guard cleanup + last compile nit)
Round-6 re-confirmed the seam a 6th time; narrow folds into T2/T6/T8:
- T2 (BLOCKER, compile): the warm error marker uses `node.id.as_str()` (was bare `node.id`; `NodeId` has no
  `Display` — same trap as T5, in both PFIX-2 and the T2 Step-3 snippet).
- T6 (MAJOR, back-compat): the warm guard fires ONLY when `Workflow + context + SessionManager`; the producer
  branches explicitly `Some((c,token)) → run_with_context_and_dispatcher` / `None → run_with_context` (cold,
  byte-identical) so no-context (and no-SM) streaming workflows stay cold.
- T6 (MAJOR, lifecycle): async scope-guard — wrap the producer body, then UNCONDITIONALLY remove
  `workflow_runs[C]` + `workflow_cancels[task]` after `.await`, so the absent-executor early-return (`:1697`)
  can't permanently busy `C` (Drop can't `.await`).
- T8 (MAJOR): added the `run_workflow_serve_flags_before_workflow_id` parser test (flags-anywhere + one positional).
- T6 (MINOR): the concurrency/cancel tests run on the `--test workflow_producer` integration target, not `--lib`.

---

## File Structure
- `crates/bridge-workflow/Cargo.toml` — `async-trait` dev→`[dependencies]` (FIX-1).
- `crates/bridge-workflow/src/executor.rs` — `WorkflowNodeDispatcher` trait + `NodeTurn`/`NodeTurnExit`/
  `NodeTurnCleanup`; the `*_and_dispatcher` entry methods that THREAD `dispatcher: Arc<dyn WorkflowNodeDispatcher>`
  as a parameter into `run_node` (PFIX-1 — NO struct field; `run_node` gains `dispatcher: Option<&Arc<dyn
  WorkflowNodeDispatcher>>`); `run_node` warm branch (cold inline UNCHANGED) + seed prepend.
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

- [ ] **Step 1 (PFIX-1 — parameter-threaded, NO struct field):** Do NOT add a `node_dispatch` field (it breaks
  the internal rebuild `WorkflowExecutor{registry:..}` at `executor.rs:222` and `with_node_dispatch(self)` can't
  move out of the server's `Arc<WorkflowExecutor>`). Instead add `run_with_context_and_dispatcher(&self, ...,
  dispatcher: Arc<dyn WorkflowNodeDispatcher>)` + `run_from_with_context_and_dispatcher(...)` that THREAD
  `dispatcher` as a parameter down to `run_node` (whose signature gains `dispatcher: Option<&Arc<dyn
  WorkflowNodeDispatcher>>`). The EXISTING cold `run_*` methods pass `None`; inside the `:222` per-node rebuild,
  pass the dispatcher into the per-node closure (do NOT store it on the struct).
- [ ] **Step 2 (tests):** (a) `cold_path_unchanged` — existing executor tests still green (the `None` branch is
  byte-identical: same `workflow-{wf}-{node}-{run_id}` id, forget at every site). (b) `warm_dispatch_no_forget`
  — a `FakeDispatcher` (None-forget) + a recording backend → assert NO `forget_session`, the node prompted on
  the dispatcher's session, `cleanup.on_exit(Normal)` ran. (c) `warm_seed_prepended` — `NodeTurn.seed=Some("S")`
  → the prompt's FIRST part is the wrapped seed. (d) `dispatcher_cancel_drains` — cancel mid-run →
  `on_exit(Canceled)` + the `FuturesUnordered` drain still completes (W3b).
- [ ] **Step 3 (impl):** in `run_node`, KEEP the existing `:76` `if cancel.is_cancelled() { return (format!(
  "[node {} canceled]", node.id.as_str()), false) }` pre-check FIRST (MINOR-2: an already-canceled warm run must
  NOT mint/claim a child session), THEN branch on the threaded param: `match dispatcher { Some(d) => WARM, None
  => COLD }`. WARM path: `let turn = match d.checkout(wf_id, node, run_id, ctx).await { Ok(t) => t,
  Err(e) => return (format!("[node {} failed: {:?}]", node.id.as_str(), e), false) };` (PFIX-2 — `run_node` returns
  `(String,bool)`, NOT `Result`; mirror the cold marker `executor.rs:98/100`, do NOT `?`/panic). Build `parts` =
  `seed`-prepended (if `turn.seed`, `parts.insert(0, Part{text: format!("[Summary of earlier context in this
  session]\n{seed}")})`) then the rendered prompt; run the prompt+drain loop on `turn.backend`/`turn.session`;
  on each exit branch call `turn.cleanup.on_exit(exit)` with the right `NodeTurnExit` (Normal / Canceled /
  Error(e)) — REPLACING the cold `forget_session` calls. **PFIX-3:** the WARM loop's cancel arm must NOT call
  `backend.cancel` (the cleanup owns cancel via `sm.cancel(child)`) — give the warm path its own loop body or an
  `is_warm` flag gating the `:137` `backend.cancel`. ELSE (`None`) → the EXISTING inline cold path UNCHANGED
  (byte-identical: same id, `backend.cancel` at `:137`, `forget` at every site). Keep the `FuturesUnordered`
  scheduler (`:322`) untouched.
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
    // PFIX-4 (FIX-2 atomicity): hold `children` ACROSS checkout_turn + insert. A concurrent
    // `*_with_children` sweep takes `children` FIRST (Task 4), so it WAITS for an in-progress
    // child checkout instead of missing it — closes the register-after-release leak window.
    // Lock order is children → by_context (checkout_turn locks by_context internally); the
    // sweeps use the same order, so no deadlock.
    let mut children = self.children.lock().await;
    let turn = self.checkout_turn(child, agent, overrides, cwd, op).await?; // existing warm reuse/mint
    children.entry(parent.clone()).or_default().insert(child.clone());      // register on SUCCESS, lock still held
    Ok(turn)
}
pub async fn expire_turn(&self, ctx: &ContextId) { self.release(ctx).await; } // backend process gone (FIX-6)
```
  (On `checkout_turn` failure the `?` returns BEFORE the insert → no stale entry, FIX-2/M3. The held
  `children` lock makes register-on-success atomic w.r.t. sweeps.)
- [ ] **Step 3:** `cargo test -p bridge-a2a-inbound --lib checkout_child && cargo test --workspace --no-run`. Commit.

---

### Task 4: SessionManager `{release,clear,cancel}_with_children` sweep (FIX-2)

**Files:** `crates/bridge-a2a-inbound/src/session_manager.rs`

- [ ] **Step 1 (tests):** `release_with_children_sweeps` — register 2 children under C; `release_with_children(C)`
  releases C (if present) + both children + removes the `children[C]` entry; tolerant of an absent parent handle
  (success). **`cancel_then_release_frees_children` (round-3 BLOCKER-1):** register 2 children; `cancel_with_children(C)`
  idles them but KEEPS `children[C]`; a subsequent `release_with_children(C)` STILL frees both. Same for
  `clear_then_release_frees_children` with `clear_with_children`. **`clear_with_children_unknown_is_not_found`
  (round-3 BLOCKER-2):** no parent handle + no children → `Ok(ResetOutcome::NotFound)`;
  `cancel_with_children_unknown_is_not_found` → `Err(SessionNotFound)`. **`clear_with_children_threads_force`
  (round-3 MAJOR-1):** a Running child + `force=true` → child reset (NOT `HandleBusy`).
  **`clear_with_children_running_child_without_force_is_busy` (round-4 MAJOR):** a Running child + `force=false` →
  `clear_with_children` returns `Err(HandleBusy)` (the real child error propagates, NOT blanket success).
  **`cancel_idle_handle_skips_backend_cancel` (round-5 BLOCKER):** checkout (Running) → `cancel` (1 backend
  cancel) → `cancel` again while Idle → STILL 1 backend cancel (the 2nd is a no-op; assert `FakeBackend.cancels()`
  len stays 1).
- [ ] **Step 2 (impl):** **FIRST (round-5 BLOCKER — idempotent `cancel`):** modify `SessionManager::cancel`
  (`:441`): after `h.state = Idle; h.op = None;`, if `!was_running` `return Ok(())` (skip the `:461`
  `backend.cancel` — no in-flight turn to cancel; matches the method's "cancel an in-flight turn" contract). This
  makes `cancel` idempotent, so `cancel_with_children(C)` AND the executor cleanup's `on_exit(Canceled) →
  sm.cancel(child)` together yield EXACTLY ONE `backend.cancel` per child (whichever runs second sees the child
  already Idle → no-op). Existing cancel tests cancel a RUNNING handle (`:1615/:1640`) so stay green. THEN add the
  three helpers. **Each LOCKS `children` FIRST and HOLDS the guard across the whole
  sweep** (PFIX-4 — waits for an in-progress `checkout_child_turn`; lock order children→by_context, no deadlock;
  tokio `MutexGuard` is `Send` so holding across the per-child `.await` is fine). Snapshot `children[C]` (CLONE,
  do NOT remove yet). **Round-3 BLOCKER-1 — only `release` frees a handle; `cancel` idles it and `reset_session`
  keeps it warm, so cancel/clear MUST RETAIN `children[C]` (else a later `release C` can't free the still-warm
  children):**
  - `release_with_children(&self, ctx)`: `self.release(ctx).await` + `self.release(child).await` for each child;
    THEN `children.remove(ctx)` (still holding the guard). Returns `()` (release is idempotent — wire success
    unchanged).
  - `cancel_with_children(&self, ctx) -> Result<(), BridgeError>`: `let p = self.cancel(ctx).await;` (Ok / Err
    SessionNotFound); `for child { let _ = self.cancel(child).await; }`; **KEEP `children[ctx]`.** Return `Ok(())`
    if `p.is_ok() || !snapshot.is_empty()` else `Err(SessionNotFound)` (round-3 BLOCKER-2 — unknown ctx stays
    not-found).
  - `clear_with_children(&self, ctx, force: bool) -> Result<ResetOutcome, BridgeError>` (round-3 MAJOR-1 — thread
    `force`; `ResetOpts` is neither `Copy` nor `Clone`, so take `force: bool` and build `ResetOpts { force }` per
    call): `let p = self.reset_session(ctx, ResetOpts { force }).await?;`; `for child { match
    self.reset_session(child, ResetOpts { force }).await { Ok(_) => {}, Err(e) => return Err(e) } }` (**round-4
    MAJOR — do NOT `let _ =`: `Ok(_)` accepts both `Cleared` and a stale child's `NotFound`, but a real error
    (`HandleBusy` from a Running child without `force`) PROPAGATES; "clear every child" must not report success
    while a child rejected the reset**); **KEEP `children[ctx]`.** Return `Ok(match p {
    ResetOutcome::Cleared { generation } => ResetOutcome::Cleared { generation }, ResetOutcome::NotFound if
    !snapshot.is_empty() => ResetOutcome::Cleared { generation: 0 }, ResetOutcome::NotFound =>
    ResetOutcome::NotFound })` (round-3 BLOCKER-2 — unknown ctx with no children stays `NotFound`; `generation: 0`
    = workflow-parent sentinel, the parent is never a handle).
- [ ] **Step 3:** `cargo test -p bridge-a2a-inbound --lib with_children && cargo test --workspace --no-run`. Commit.

---

### Task 5: `WarmWorkflowNodeDispatcher` in bridge-a2a-inbound (FIX-2/5/6/7)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`

- [ ] **Step 1 (tests):** `warm_workflow_dispatch_checks_out_child` — derives the child ctx
  `<C>::workflow::<wf>::node::<node>`, `checkout_child_turn`, returns `NodeTurn{seed}`; `on_exit(Normal)` →
  `finish_turn` (NOT forget); `on_exit(Canceled)` → `sm.cancel(child)`; `on_exit(Error(AgentCrashed))` →
  `sm.expire_turn(child)`; `on_exit(Error(other))` → `finish_turn`.
- [ ] **Step 2 (impl):** add `struct WarmWorkflowNodeDispatcher { sm: Arc<SessionManager>, parent: ContextId,
  cwd: Option<SessionCwd> }` implementing `WorkflowNodeDispatcher::checkout` (note: `checkout` takes `&self`, so
  every field is borrowed — `&self.parent`, `self.cwd.clone()`, `self.sm` — round-5 MAJOR: you CANNOT move
  `self.cwd` out of `&self`): derive `child = ContextId::parse(format!("{}::workflow::{}::node::{}",
  self.parent.as_str(), wf_id, node.id.as_str()))?` (BLOCKER — `ContextId`/`NodeId` are String newtypes with
  `as_str()` and NO `Display`; `wf_id` is already `&str`; the `::`-containing string parses fine because
  `ContextId` uses the non-strict `id_newtype!` macro); mint the `op` as `OperationId::parse(format!(
  "workflow-{run_id}-node-{}", node.id.as_str()))?` (round-4 MINOR — `run_id` is already `&str` so `{run_id}`
  inlines, but `NodeId` has no `Display` → `.as_str()`); `let turn = self.sm.checkout_child_turn(&self.parent,
  &child, node.agent.clone(), None, self.cwd.clone(), op).await?` (round-5 MAJOR — `checkout_child_turn` takes an
  OWNED `Option<SessionCwd>`, so `self.cwd.clone()`, mirroring `warm_local_dispatch`'s `routed.session_cwd.clone()`
  at `server.rs:623`);
  return `NodeTurn{ backend: turn.backend, session: turn.session, seed: turn.seed, cleanup: Box::new(WarmNode
  Cleanup{ sm: self.sm.clone(), child, gen: turn.generation, op: turn.op }) }`. The `WarmNodeCleanup::on_exit` matches
  `NodeTurnExit` per FIX-6/7 (Normal/Error(other)→`finish_turn(child,gen,&op)`; Canceled→`sm.cancel(child)`;
  Error(AgentCrashed)→`sm.expire_turn(child)`).
- [ ] **Step 3:** `cargo test -p bridge-a2a-inbound --lib warm_workflow && cargo test --workspace --no-run`. Commit.

---

### Task 6: Workflow-run guard + spawn_workflow_producer warm wiring + SessionCancel scheduler-cancel (FIX-3)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`

- [ ] **Step 1 (tests):** `concurrent_same_context_workflow_handle_busy` — two streaming workflow sends on C →
  the 2nd returns `HandleBusy` (JSON-RPC) early; `session_cancel_cancels_workflow_run` — `SessionCancel C`
  cancels the parent run token (the executor stops scheduling) + `cancel_with_children(C)`;
  `session_cancel_workflow_parent_sweeps_children` (PFIX-5) — AFTER the workflow run has finished (token already
  removed from `workflow_runs`), `SessionCancel C` STILL sweeps the registered children (no token to cancel, but
  `cancel_with_children(C)` runs unconditionally); `session_cancel_keeps_context_busy_until_producer_exits`
  (round-4 BLOCKER) — while a run is DRAINING after `SessionCancel C` (token cancelled but producer not yet
  exited), a 2nd same-context workflow send STILL gets `HandleBusy` (the guard is NOT removed by `session_cancel`);
  `active_session_cancel_one_backend_cancel_per_child` (round-5 BLOCKER) — an ACTIVE workflow `SessionCancel C`
  (token cancel + `cancel_with_children`) PLUS the executor cleanup's `on_exit(Canceled)→sm.cancel(child)` yields
  EXACTLY ONE `backend.cancel` per child (assert via the gated workflow backend's cancel count), proving the
  idempotent-`cancel` fix.
- [ ] **Step 2 (impl):** add `workflow_runs: Mutex<HashMap<ContextId, CancellationToken>>` to `InboundServer`.
  **round-5 MAJOR — guard BEFORE `store.put`:** `stream_message` persists `task→session` at `server.rs:769`
  BEFORE the route match, so a guard placed in the Workflow arm (`:823`) would let a rejected 2nd workflow mutate
  `SessionStore`. Place the guard RIGHT AFTER `gate()` (`:763`) and BEFORE the `:769` `store.put` — **and ONLY when a
  SessionManager exists** (round-6 MAJOR — no SM ⇒ stay cold; the `with_workflows`-without-SM integration tests
  must not be warmed): `let workflow_token = if matches!(routed.target, RouteTarget::Workflow(_)) &&
  routed.context_id.is_some() && srv.session_manager.is_some() { let c = routed.context_id.clone().unwrap(); let
  mut runs = srv.workflow_runs.lock().await; if runs.contains_key(&c) { return bridge_err_to_jsonrpc(id,
  &BridgeError::HandleBusy); } let t = CancellationToken::new(); runs.insert(c.clone(), t.clone()); Some((c, t)) }
  else { None };` (the `HandleBusy` returns BEFORE any `store.put` → no state mutation; the cold
  no-context workflow gets `None` → unchanged). Thread `workflow_token: Option<(ContextId,
  CancellationToken)>` into the Workflow arm → `spawn_workflow_producer`. **round-6 MAJOR — explicit warm/cold
  branch + async scope-guard cleanup** (Drop can't `.await`, and the absent-executor/graph early-return at
  `:1697` must NOT strand `workflow_runs[C]`): wrap the producer body so cleanup ALWAYS runs:
```rust
tokio::spawn(async move {
    let _ = async {
        // existing executor/graph resolve + early-return on absent (:1695-1701) ...
        let token = match &workflow_token { Some((_, t)) => t.clone(), None => CancellationToken::new() };
        srv.workflow_cancels.lock().await.insert(task.clone(), token.clone()); // backs CancelTask :2832
        let stream = match &workflow_token {
            Some((c, _)) => executor.run_with_context_and_dispatcher(  // WARM (PFIX-1, on &WorkflowExecutor)
                graph, input, task.as_str().into(), token, wf_ctx,
                Arc::new(WarmWorkflowNodeDispatcher {
                    sm: srv.session_manager.clone().unwrap(),
                    parent: c.clone(), cwd: routed.session_cwd.clone(),
                })),
            None => executor.run_with_context(graph, input, task.as_str().into(), token, wf_ctx), // COLD, byte-identical
        };
        // drain + terminal fallback — UNCHANGED (:1720-1730)
    }.await;
    // UNCONDITIONAL cleanup on EVERY exit (early-return or normal) — no permanently-busy C:
    srv.workflow_cancels.lock().await.remove(&task);
    if let Some((c, _)) = &workflow_token { srv.workflow_runs.lock().await.remove(c); }
});
```
  The `None` arm keeps no-context streaming workflows on the cold `run_with_context` path (back-compat — the
  `with_workflows`-without-SM tests stay green); the post-`.await` block removes BOTH maps even on the early
  failure (mirrors `workflow_cancels` :1711/:1731; note the `routed.task`/`session_cwd` move order :1690/:1716). In `session_cancel`
  (`server.rs:3104`): **round-4 BLOCKER — GET, do NOT remove the guard.** `let token = { workflow_runs.lock()
  .await.get(C).cloned() };` (clone the token, LEAVE the entry — the guard is released ONLY on producer exit,
  mirroring `workflow_cancels`; removing it here lets a 2nd same-context run pass the guard and re-claim a child
  that `SessionManager::cancel` already marked Idle at `:454` but is still tearing down via `backend.cancel` at
  `:461`). `if let Some(t) = &token { t.cancel(); }` (stop the scheduler if a run is active); THEN `let swept =
  sm.cancel_with_children(C).await;` UNCONDITIONALLY (PFIX-5/FIX-11 — sweeps children even AFTER the run
  finished). Respond OK if `token.is_some() || swept.is_ok()`; else map `swept`'s `Err(SessionNotFound)` to the
  wire error (round-3 BLOCKER-2 — an active run token counts as success even with zero minted children). Do NOT
  fall back to bare `sm.cancel(C)`. (The producer exit path removes `workflow_runs[C]` + `workflow_cancels[task]`.)
- [ ] **Step 3:** **round-6 MINOR — the concurrency/cancel tests live in the `tests/workflow_producer.rs`
  INTEGRATION target (the `with_workflows` harness), NOT `--lib`:** `cargo test -p bridge-a2a-inbound --test
  workflow_producer 'workflow_handle_busy|session_cancel|one_backend_cancel' && cargo test -p bridge-a2a-inbound
  --lib session_cancel_cancels && cargo test --workspace --no-run`. Commit.

---

### Task 7: gate lift (streaming-only) + unary reject + sweep wire handlers (FIX-8/10/11)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`

- [ ] **Step 1 (tests):** `gate_allows_contextid_on_workflow_streaming`; `gate_rejects_unary_workflow_contextid`;
  `gate_still_rejects_delegate_fanout_contextid`; `session_release_workflow_parent_sweeps_children` (release C
  on a workflow parent succeeds + frees children); `session_clear_unknown_context_still_not_found` (round-3
  BLOCKER-2 — an unknown ctx with no children + no handle STILL returns HTTP 400 `SessionNotFound`, preserving
  the existing test at `server.rs:6955`).
- [ ] **Step 2 (impl):** in `gate()` (`:352`) change the rejection to `if context_id.is_some() &&
  !matches!(target, RouteTarget::Local(_) | RouteTarget::Workflow(_))` (FIX-10). In the UNARY `SendMessage`
  path, reject `routed.context_id.is_some() && matches!(target, Workflow)` (unary workflow = detached, deferred
  — FIX-8). **MAJOR-2: place this reject immediately after the successful `gate()` and BEFORE `srv.store.put`
  (`server.rs:2371`)** — return the JSON-RPC error before ANY task/session store write, so a rejected request
  never mutates state. Rewire the handlers (`server.rs:3005/3030`): `SessionRelease` → `sm.release_with_children(ctx)` (returns `()`,
  always `released:true` — unchanged, release is idempotent). `SessionClear` → `sm.clear_with_children(ctx,
  force)` (round-3 MAJOR-1 — thread the already-parsed `force` bool) and **KEEP the existing outcome mapping**
  (`Ok(Cleared{generation})`→ok; `Ok(NotFound)`→`SessionNotFound` error; `Err(e)`→jsonrpc). Round-3 BLOCKER-2:
  the helper returns `Cleared` whenever children were swept and `NotFound` ONLY for an unknown ctx with no
  children, so an unknown-clear still 400s (existing test green) while a workflow parent succeeds. (The
  `SessionCancel` sweep + token-success is in T6.)
- [ ] **Step 3:** `cargo test -p bridge-a2a-inbound --lib 'gate_|session_release_workflow' && cargo test
  --workspace --no-run`. Commit.

---

### Task 8: CLI serve-client (FIX-11)

**Files:** `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1 (tests):** `run_workflow_context_requires_serve` (`--context` without `--serve` → error);
  `run_workflow_config_rejected_with_serve` (`--config` + `--serve` → error, detected on the un-defaulted
  Option); `run_workflow_serve_flags_before_workflow_id` (round-6 MAJOR / PFIX-8 — parse `run-workflow --serve
  --context C <wf>`: flags consumed ANYWHERE, exactly ONE non-flag token = the workflow id; the current parser
  assumes arg-0 is the id at `main.rs:543`); `serve_client_builds_streaming_message` (the message map has
  `contextId`, `metadata ["a2a-bridge.skill"]=<wf>`, parts).
- [ ] **Step 2 (impl):** extend `parse_run_workflow_args` (`:543`) with `--serve` (bool), `--url`
  (default `http://127.0.0.1:8080`), `--context` (Option); reject `--context` without `--serve`; reject explicit
  `--config` with `--serve` (check the raw Option BEFORE the `CONFIG_PATH` default). In `run_workflow_cmd`
  (`:2233`): **MAJOR-2 — the `--serve` branch must EARLY-RETURN before the local config read (`main.rs:2242`) and
  any workflow lookup** (the workflow lives in the running serve's config, not locally). Right after parsing args
  + reading the input file: if `--serve` → build a `SendStreamingMessage` message map (`message.contextId=C`,
  `metadata["a2a-bridge.skill"]=<wf>`, parts=input, cwd from `--session-cwd`) — its OWN map, NOT via
  `submit_cmd`'s skill-guesser (PFIX-7) — POST to `--url`, then drain the SSE byte-stream (the `task_watch_cmd`
  loop is INLINE `:2756`, no parser fn — extract/duplicate it), JSON-parsing each `data:` frame as
  `a2a::StreamResponse` to find the terminal `StatusUpdate{state∈{Completed,Failed,Canceled}}` (exit code) +
  collect `ArtifactUpdate` text (stdout/`--out`); statuses to stderr. ELSE the existing local one-shot path
  (snapshot/lease/spawn/registry/executor `:2282-2358`) UNCHANGED.
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
