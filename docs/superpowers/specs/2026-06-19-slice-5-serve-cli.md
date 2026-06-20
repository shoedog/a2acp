# Slice 5 — Serve-backed `run-workflow` + handle-aware keep-warm — Spec

> **Status:** v2 (dual spec-review folded). Drafted from the dual-lens analysis, then dual spec-reviewed
> (codex-xhigh + Opus, both `fix-then-plan`, layering inversion CONFIRMED sound) — FIX-1..11 below are BINDING.
> Next: plan → dual plan-review. **The FIX list supersedes any contradicting body text in §2/§4/§5/§8.**
>
> **Roadmap:** Slice 5 — the **MVP CUT-LINE** (closes Slices 0–5). Authoritative scope:
> `docs/superpowers/specs/2026-06-17-orchestration-slicing.md` row 5.

## Goal

Make `run-workflow` usable in a WARM loop. **`run-workflow --serve [--url U] --context C <wf> --input <f>`**
makes the CLI a SERVE CLIENT so a workflow's agent sessions stay warm in the long-running serve and **REUSE
across `run-workflow` invocations — no cold respawn on the 2nd run**. Handle-aware executor keep-warm (**no
per-node forced `forget_session` for handle-backed runs**); **drain-on-cancel preserved** (W3b); the **non-serve
(local) path is byte-identical (back-compat)**.

## KEYSTONE constraints (do not violate)
- **Layering:** `WorkflowExecutor` (`bridge-workflow`) is BELOW `SessionManager` (`bridge-a2a-inbound`, which
  depends on `bridge-workflow`). The executor MUST NOT import `SessionManager`. The seam is a
  **dependency-inversion trait** the executor defines + the inbound layer implements.
- **Back-compat:** the local `run-workflow` path and the cold node behavior MUST stay observably unchanged.

## v2 — dual spec-review fixes folded (BINDING)

Both reviewers returned `fix-then-plan` and CONFIRMED the layering inversion is sound (no `bridge-workflow →
bridge-a2a-inbound` edge; the trait uses only bridge-core types). The gaps are operational. These FIXes are
binding:

- **FIX-1 (compile) — `async-trait` is DEV-only in `bridge-workflow/Cargo.toml`** (`:16`); the production
  trait needs it under `[dependencies]` (`async-trait.workspace = true`). A planned Cargo change.
- **FIX-2 (BLOCKER — the keystone) — `SessionManager` OWNS child lifecycle, atomically.** Add
  `checkout_child_turn(parent: &ContextId, child: &ContextId, agent, cwd, op) -> Result<WarmTurn>` that, under
  the manager lock, REGISTERS `parent→child` (a `children: Mutex<HashMap<ContextId, HashSet<ContextId>>>`) **on
  checkout SUCCESS only** AND returns the `WarmTurn` (carrying the exact `generation` + `op`). The warm cleanup
  MUST `finish_turn(child, turn.generation, &turn.op)` with that EXACT op/gen — else it silently no-ops
  (`session_manager.rs:370-379`), strands the child `Running`, and the next run gets `HandleBusy` instead of
  warm reuse → **the DoD is defeated** (Opus B2/M3). The dispatcher does NOT keep its own mutex/registration
  (race-prone); it just calls `checkout_child_turn`. Lifecycle helpers `release_with_children(C)` /
  `clear_with_children(C)` / `cancel_with_children(C)` sweep registered children under the manager lock
  (tolerant of already-absent children); the wire handlers call these.
- **FIX-3 (BLOCKER — concurrency + cancel-the-scheduler) — a parent-context WORKFLOW-RUN GUARD** distinct from
  the child warm sessions. In the `RouteTarget::Workflow` STREAMING dispatch (`spawn_workflow_producer`),
  BEFORE returning SSE: insert `parent C → CancellationToken` into a `Mutex<HashMap<ContextId, Cancellation
  Token>>` of in-flight workflow runs; **if `C` is already present → reject early with `HandleBusy`** (JSON-RPC,
  not a mid-run node failure — Opus B3). Release on producer exit (mirror `workflow_cancels`
  `server.rs:1711/1731`). **`SessionCancel C` cancels that parent workflow `CancellationToken`** (stopping the
  executor's scheduler — `executor.rs:322` only stops on its token; cancelling children alone leaves downstream
  nodes scheduling — codex BLOCKER), THEN `cancel_with_children(C)`. The executor's drain then preserves W3b.
- **FIX-4 (back-compat keystone, O1) — keep the COLD path INLINE; do NOT unify into a `ColdWorkflowNode
  Dispatcher`.** `run_node` keeps today's inline cold path BYTE-IDENTICAL (configure errors ignored
  `executor.rs:104`, forget at every site, session id `workflow-{wf}-{node}-{run_id}`); the WARM path is an
  ADDITIONAL branch. `WorkflowExecutor` gains `Option<Arc<dyn WorkflowNodeDispatcher>>` (None = inline cold,
  back-compat; Some = warm). The trait is ONLY the warm seam. The existing executor tests lock cold behavior.
- **FIX-5 (O3) — ONE cleanup method `on_exit(self: Box<Self>, exit: NodeTurnExit)`** where `NodeTurnExit ∈
  {Normal, Canceled, Error(BridgeError)}` — NOT three methods. It carries the error for classification (FIX-6).
  Each impl closes over what it owns (cold: backend+session; warm: `Arc<SessionManager>`+child+gen+op) — no
  borrowed backend/session params.
- **FIX-6 (O2) — warm error classification, ONE match arm:** `on_exit(Error(e))` → if `e` is
  `BridgeError::AgentCrashed{..}` (the backend process is gone) EXPIRE the child (`sm.release(child)` /
  `sm.expire_turn(child)`); else `finish_turn(child, gen, op)` (keep warm — the next run reconciles or the
  operator releases). `on_exit(Normal)` → `finish_turn`; `on_exit(Canceled)` → see FIX-7.
- **FIX-7 (M1) — warm CANCEL uses `SessionManager::cancel(child)`** (= `backend.cancel` + Idle-or-defer,
  preserving the claim-defer/ABA invariant `session_manager.rs:441-462`), NOT a raw `backend.cancel` +
  `finish_turn` (which bypasses the invariant). So `on_exit(Canceled)` → `sm.cancel(child)` (do NOT also
  finish — cancel already idles).
- **FIX-8 (codex MAJOR, O5) — the gate lift is STREAMING-ONLY.** `gate()` is shared by unary + streaming; lift
  contextId for `RouteTarget::Workflow(_)` but **REJECT unary `SendMessage` + Workflow + contextId** (unary
  workflow = detached/durable submit `server.rs:2367/2489`, explicitly deferred). Add the explicit unary
  rejection (`routed.context_id.is_some() && Workflow` on the unary path → error).
- **FIX-9 (M4) — §1 OUT addition:** warm workflow nodes do NOT record usage (the executor drops `Update::Usage`
  `executor.rs:143`); per-child usage telemetry / `warm_usage_warn_fraction` for workflow nodes is deferred.
  Documented asymmetry with the single-agent warm path.
- **FIX-10 (m3) — gate match arm:** `if context_id.is_some() && !matches!(target, RouteTarget::Local(_) |
  RouteTarget::Workflow(_))` (`server.rs:352`); a test asserts Delegate/Fanout still reject.
- **FIX-11 (m4/m5/m6) — wire/CLI exactness:** `release/clear/cancel C` on a workflow PARENT sweeps children +
  treats the (always-absent) parent handle as SUCCESS, not `SessionNotFound`; the CLI builds its OWN
  `SendStreamingMessage` map (NOT `submit_cmd`'s positional skill-guesser `main.rs:2616`), setting
  `metadata["a2a-bridge.skill"]=<wf>` from the parsed workflow-id; `--config`+`--serve` co-occurrence is
  detected on the UN-defaulted Option (before the `CONFIG_PATH` default, `main.rs:593`).

## 1. Scope

**IN:**
- A `WorkflowNodeDispatcher` trait (in `bridge-workflow`) + `NodeTurn`/`NodeTurnCleanup`; a `ColdWorkflowNode
  Dispatcher` (resolve+configure+forget = today) + dispatcher-aware executor methods.
- A `WarmWorkflowNodeDispatcher` (in `bridge-a2a-inbound`) over `SessionManager`: per-node `checkout_turn` keyed
  by a derived child context, `finish_turn` (no forget), seed injection.
- Per-node child context derivation `<parent>::workflow::<wf_id>::node::<node_id>`.
- Parent→child tracking so `SessionRelease/Clear/Cancel C` sweep the workflow's children.
- The `gate()` contextId lift for `RouteTarget::Workflow(_)` (streaming path); concurrent same-context run →
  `HandleBusy`.
- CLI: `run-workflow --serve [--url] --context C` streaming client.

**OUT:**
- Detached/durable (W3a/W3b) warm workflow runs (streaming-only MVP; the persisted-contextId + task-vs-handle
  SEQ-AUTHORITY on C is deferred).
- Multi-agent warm-POOL beyond per-node child contexts; `SessionCompact C` on a workflow context (N/A — a
  workflow context is a namespace, not one session); model/effort overrides into warm workflow nodes (executor
  passes `None` today — keep).
- MCP surface (S8).

## 2. The `WorkflowNodeDispatcher` seam (dependency inversion)

In `bridge-workflow` (next to the executor; uses only `bridge-core` types):
```rust
#[async_trait::async_trait]
pub trait WorkflowNodeDispatcher: Send + Sync {
    /// Acquire a (backend, session) for this node — cold (fresh) or warm (reused) — + a cleanup the executor
    /// runs after prompt+drain. `seed` (if any) is prepended to the node's prompt parts by the executor.
    async fn checkout(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        run_id: &str,
        ctx: &WorkflowRunContext,
    ) -> Result<NodeTurn, BridgeError>;
}
pub struct NodeTurn {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
    pub seed: Option<String>,              // Slice-4 pending_seed for this child context (warm only)
    pub cleanup: Box<dyn NodeTurnCleanup>, // runs on normal / cancel / err exit
}
#[async_trait::async_trait]
pub trait NodeTurnCleanup: Send {
    async fn on_normal(self: Box<Self>);                  // cold: forget; warm: finish_turn
    async fn on_cancel(self: Box<Self>, backend: &dyn AgentBackend, session: &SessionId); // both: backend.cancel then (cold forget / warm finish)
    async fn on_error(self: Box<Self>);                   // cold: forget; warm: finish_turn (or expire if unrecoverable — §5)
}
```
- **`ColdWorkflowNodeDispatcher { registry }`** (in `bridge-workflow`): `checkout` = `registry.resolve` +
  `configure_session` (session = `workflow-{wf}-{node}-{run_id}`, as today), `seed=None`, cleanup = forget at
  every site. **The existing `run_with_context`/`run_from_with_context` delegate to this** — so cold behavior is
  preserved.
- **`WarmWorkflowNodeDispatcher { sm: Arc<SessionManager>, parent: ContextId }`** (in `bridge-a2a-inbound`):
  `checkout` derives the child context (§3), calls `sm.checkout_turn(child, node.agent, None, cwd, op)` (warm
  reuse), returns `seed = turn.seed`, cleanup = `finish_turn` (no forget). Registers the child (§4).
- **Executor change:** add `run_from_with_context_and_dispatcher(graph, input, run_id, cancel, seed, ctx,
  dispatcher: Arc<dyn WorkflowNodeDispatcher>)`; `run_node` takes the dispatcher, calls `dispatcher.checkout`
  instead of inlining resolve/configure, **prepends `node_turn.seed` to the prompt parts**, runs the SHARED
  prompt+drain loop (`executor.rs:131-152`, untouched), then invokes `cleanup.on_*`. The existing methods pass
  a `ColdWorkflowNodeDispatcher`. The `FuturesUnordered` scheduler + drain-on-cancel (`:322`) is UNTOUCHED.
- **Back-compat decision (spec/plan-review to confirm):** the cold path becomes a `ColdWorkflowNodeDispatcher`
  so `run_node` has ONE path — REQUIRES a test proving the cold node behavior (resolve→configure→prompt→
  forget, same session id) is byte-identical. If that's risky, keep the cold path inline + add the warm path
  behind the dispatcher; pick in v2.

## 3. Per-node child context keying

`child = ContextId::parse("<parent>::workflow::<wf_id>::node::<node_id>")`. STABLE across runs (no `run_id` →
reuse). Per-NODE (two same-agent nodes keep distinct histories). Includes `wf_id` (one parent context can host
multiple workflows). `ContextId` is charset-unvalidated (`ids.rs:34`) so `::` is safe; the derived id is just
another `ContextId` → the existing `checkout_turn` warms it with backend_session `ctx-<child>-g0`.

## 4. Parent→child tracking + lifecycle (NOT prefix matching)

- `SessionManager` gains a `children: Mutex<HashMap<ContextId, HashSet<ContextId>>>` (parent → children). The
  warm workflow checkout REGISTERS `parent → child` before/at `checkout_turn`.
- `SessionRelease C` / `SessionClear C` / `SessionCancel C` operate on exact `C` **plus every registered
  child** (release/clear/cancel each child; clear the parent→children entry on release). (A new internal
  `release_with_children`/`for_each_child` helper; the wire handlers call it.)
- **Prefix matching is FORBIDDEN** (ContextId unvalidated → collisions). Tracking only.
- TTL reap remains the idle backstop (children go idle after a run, reaped after `warm_idle_ttl`).
- `SessionCompact C` / `SessionStatus C` on a workflow PARENT = N/A for MVP (a parent isn't a single warm
  handle); per-CHILD status/compact works (the child IS a normal warm context). Document.

## 5. forget→finish per-site map (warm path)

| executor.rs site | cold (unchanged) | warm |
|---|---|---|
| `:115` cancel after configure | `forget_session` | `finish_turn` (no backend cancel — no stream) |
| `:122` cancel before prompt | `forget_session` | `finish_turn` (no backend cancel) |
| `:127` prompt error | `forget_session` | `finish_turn`; **classify** — an unrecoverable error class (e.g. `AgentCrashed`) EXPIREs the child instead (O-class, §11) |
| `:137` stream cancel | `backend.cancel` (+ forget) | `backend.cancel` then `finish_turn` (warm cancel; W3b drain intact) |
| `:148` stream err | (falls to `:153` forget) | `finish_turn` (mirrors `WarmTurnGuard` `server.rs:514`) |
| `:153` normal exit | `forget_session` | `finish_turn` → Idle (keep warm) |

The warm cleanup is the `NodeTurnCleanup` the warm dispatcher returned; the executor stays backend-agnostic.

## 6. Seed injection (Slice-4 integration)

If `NodeTurn.seed` is `Some` (a prior `compact` on that child left a pending summary), the executor PREPENDS a
wrapped seed `Part` to the node's prompt parts (mirror the single-agent dispatch's
`"[Summary of earlier context in this session]\n{seed}"` prepend) BEFORE `backend.prompt`. Cold dispatcher
returns `seed=None` → no-op.

## 7. CLI client

- `run-workflow <wf> --input <f> [--session-cwd R] [--config F] [--out O]` — UNCHANGED local one-shot.
- `run-workflow --serve [--url U] --context C <wf> --input <f> [--session-cwd R] [--out O]` — client:
  - `--context` **REQUIRES `--serve`** (error if `--context` without `--serve` — no accidental remote exec).
  - `--config` is **rejected with `--serve`** (the workflow lives in the running serve's config; the client
    only names the workflow-id).
  - POST `SendStreamingMessage` to `U` (default `http://127.0.0.1:8080`): `message.contextId=C`,
    `metadata["a2a-bridge.skill"]=<wf>`, parts=input, `metadata` cwd from `--session-cwd`. Stream SSE; print
    artifact text to stdout (+ `--out`), statuses to stderr; exit code from the terminal state. (Reuse
    `rpc_call`/`collect_sse`/the `task watch` SSE parser.)

## 8. gate() lift + concurrency

- Lift the Slice-0 rejection (`server.rs:352`) to allow `context_id` on **`RouteTarget::Workflow(_)`** (keep
  Delegate/Fanout rejected). The parent `C` is never itself a warm handle (only `::workflow::` children are),
  so SEQ-AUTHORITY on `C` is not violated.
- **Concurrent same-context workflow runs** must be rejected early (`HandleBusy`) or serialized — two runs on
  `C` would collide on the child checkouts; surface it at dispatch, not as node-level failure.

## 9. Failure modes

| Mode | Handling |
|---|---|
| node prompt error (warm) | `finish_turn` (keep child warm); unrecoverable class → expire the child (§5/§11). |
| node cancel (warm) | `backend.cancel` + `finish_turn`; the executor's drain (FuturesUnordered) completes (W3b). |
| concurrent `--context C` runs | early `HandleBusy`. |
| child leak | parent→child tracking + `release C` sweep; TTL backstop. |
| `--context` without `--serve` | CLI error. |
| `--config` with `--serve` | CLI error (config is serve-side). |
| non-serve path | byte-identical (cold dispatcher). |

## 10. DoD + live-gate

**DoD:** two `run-workflow --serve --context C <wf>` calls reuse the workflow's warm agents (no cold spawn on
the 2nd); the non-serve path is unchanged.

**Live-gate** (real serve + a multi-node workflow, e.g. `code-review`/`design`, codex+claude, large TTL):
1. `serve --config <multi-agent-workflow-config>`; record serve's agent-child pids = none yet.
2. `run-workflow --serve --url U --context C <wf> --input <f>` (run #1) — agents cold-spawn; child contexts
   `C::workflow::<wf>::node::*` MINT (gen 0); record agent pids + child `backend_session`s.
3. `run-workflow --serve --url U --context C <wf> --input <f>` (run #2) — **child checkouts HIT** (reuse, same
   `backend_session`), **no 2nd agent spawn** (same pids), sub-second agent start. (The felt win.)
4. `session release C` → the child warm agents freed (pids gone) [parent-child sweep].
5. `run-workflow <wf> --input <f>` (no `--serve`) — local path unchanged.

## 11. Open questions — RESOLVED by the dual spec-review (both lenses converged)
- **O1 → keep cold INLINE** (FIX-4); back-compat keystone; don't unify.
- **O2 → finish; expire ONLY `AgentCrashed`-class** via `sm.release/expire_turn(child)`; one match arm (FIX-6).
- **O3 → ONE `on_exit(self, NodeTurnExit{Normal|Canceled|Error(BridgeError)})`** (FIX-5); each impl closes over
  what it owns (no borrowed backend/session).
- **O4 → child map on `SessionManager`; the manager registers inside an atomic `checkout_child_turn`** (not the
  dispatcher, not a separate mutex) (FIX-2).
- **O5 → `SendStreamingMessage` SSE is sufficient; reject unary/detached warm-workflow context** until
  W3-warm-detached is designed (FIX-8).

No open questions remain. SPEC VERDICT (both lenses): `fix-then-plan`; all fixes folded → ready to plan.

## Test plan (TDD)
- `bridge-workflow`: `cold_dispatcher_matches_legacy_behavior` (byte-identical session id + forget); a fake
  `WorkflowNodeDispatcher` drives `run_with_context_and_dispatcher` (warm path: no forget, seed prepended);
  `dispatcher_cancel_drains` (drain-on-cancel preserved). Cold-path regression (existing executor tests green).
- `bridge-a2a-inbound`: `WarmWorkflowNodeDispatcher` derives the child context + checks out warm + finish (not
  forget); `release_C_sweeps_children`; gate allows contextId on Workflow + rejects Delegate/Fanout;
  concurrent-run `HandleBusy`.
- `bin/a2a-bridge`: `run-workflow --context` without `--serve` errors; `--config` with `--serve` errors; the
  client builds the right `SendStreamingMessage` (contextId + skill + parts).
- Live-gate: the §10 two-run warm-reuse + back-compat.
