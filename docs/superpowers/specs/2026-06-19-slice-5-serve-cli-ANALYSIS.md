# Slice 5 (Serve-backed run-workflow + keep-warm) — Architecture Analysis (pre-spec)

> Pre-spec design analysis. Two lenses fold here: **this = the Opus lens** (code-grounded); the **codex
> gpt-5.5 xhigh lens** runs in parallel (`prompts/slice-5-arch-analysis.md` + `/tmp/slice5-arch-brief.md`) and
> folds below. Becomes the input to the formal spec `docs/superpowers/specs/2026-06-19-slice-5-serve-cli.md`.
>
> **Goal (MVP cut-line, Slices 0–5):** make `run-workflow` usable in a WARM loop — `run-workflow --serve
> --context C <wf>` makes the CLI a serve client so a workflow's agent sessions stay warm in the long-running
> serve and REUSE across invocations (no cold respawn on the 2nd run). Handle-aware executor keep-warm (no
> per-node forced `forget_session` for handle-backed runs); drain-on-cancel preserved; non-serve path unchanged.

## 0. The KEYSTONE constraint — crate layering

`WorkflowExecutor` lives in **`bridge-workflow`**; `SessionManager` lives in **`bridge-a2a-inbound`**, which
DEPENDS on `bridge-workflow` (it calls the executor). So **the executor CANNOT import `SessionManager`** — that
would invert/cycle the layering. Therefore the handle-aware seam MUST be an abstraction the executor (or
`bridge-core`) DEFINES and the inbound layer IMPLEMENTS. This single fact shapes the whole design: Slice 5 is a
**dependency-inversion seam**, not "pass the SessionManager into the executor."

## 1. CLI-as-serve-client (Q1) — confirmed required

Local keep-warm across `run-workflow` invocations is **impossible**: each `run-workflow` is a fresh OS process;
a warm session lives inside a process and dies with it. So the warm session MUST live in a long-running serve,
and `--serve` (the CLI as a serve client) is REQUIRED for the cross-invocation DoD. **`run-workflow --serve
[--url U] --context C <wf> --input <f>`** POSTs the workflow as an A2A message (skill = workflow-id, contextId
= C, input as the message parts) to the serve, which routes to the Workflow target → `spawn_workflow_producer`
(`server.rs:1683`) → the executor, and streams the workflow events (SSE) back to the CLI. Reuses the EXISTING
serve-side workflow path; the CLI is a thin streaming client (mirror `submit`/the SSE client + `rpc_call`
`main.rs:2571`).

## 2. Per-node warm keying (Q2) — RECOMMEND derive `{C}::{node_id}` (Option A)

A workflow has N nodes/agents; `SessionManager` keys by ONE `ContextId`. **Derive a per-node warm context from
the parent** — each node checks out a warm session keyed by a STABLE derived id (the `run_id` dropped, so it's
the same across runs): e.g. `wf-{C}-{node_id}` (a valid `ContextId`; confirm the `ContextId` charset accepts
the chosen join — `executor.rs:80` currently mints a `SessionId` with `{wf}-{node}-{run_id}`, so a `-` join is
charset-safe). Then:
- node `review` in workflow run #1 mints/warms `wf-C-review`; run #2 REUSES `wf-C-review` → no cold start. ✅
  Works for multi-agent workflows (codex+claude→synth each get their own warm sub-context).
- **NO SessionManager keying change** (it stays `ContextId`-keyed) — the derived id IS a `ContextId`. This is
  the minimal-surface choice.
- Rejected (B) extend SM keying to `(ContextId, NodeId)` — ripples the whole `by_context` map + every lifecycle
  method for no gain over (A). Rejected (C) single-agent-only — doesn't deliver the felt win (the real loop is
  multi-agent `code-review`/`design`).

## 3. The executor↔SessionManager seam (Q3) — RECOMMEND a `NodeDispatch` trait (dependency inversion)

Define in **`bridge-workflow`** (or `bridge-core::ports`):
```rust
#[async_trait]
pub trait NodeDispatch: Send + Sync {
    /// Acquire a (backend, session) for this node — warm (reused) or cold (fresh) — plus a NodeLease whose
    /// cleanup the executor invokes after prompt+drain. Cold impl = resolve+configure+forget; warm impl =
    /// checkout_turn(derived ctx)/finish_turn, NO forget.
    async fn acquire(&self, node_id: &NodeId, agent: &AgentId, cwd: Option<SessionCwd>)
        -> Result<NodeLease, BridgeError>;
}
pub struct NodeLease {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
    pub cleanup: NodeCleanup, // enum/boxed: Forget(session) | Finish{sm-callback} ; runs on normal/cancel/err
}
```
- **`WorkflowExecutor`** gains an optional `node_dispatch: Option<Arc<dyn NodeDispatch>>` (builder
  `with_node_dispatch`); `new(registry)` leaves it `None`. **When `None` → today's run_node path runs
  BYTE-IDENTICALLY** (resolve+configure+forget) — back-compat guaranteed by leaving the cold path untouched.
  When `Some` → `run_node` acquires via the dispatch + uses `cleanup` instead of inline forget.
- The **cold default** can also be expressed as a `RegistryDispatch` impl living in the executor (so run_node
  has ONE path), but to GUARANTEE back-compat I lean: keep the existing inline cold path, add the warm path
  behind the `Option` — smaller blast radius, the non-serve path provably unchanged.
- **The warm `NodeDispatch` impl lives in `bridge-a2a-inbound`** (it can see `SessionManager`): it derives
  `wf-{C}-{node_id}`, calls `checkout_turn` (warm reuse) + returns a `cleanup` that `finish_turn`s (→Idle, NOT
  forgotten), mirroring `WarmTurnGuard` (`server.rs:506`). The serve constructs the executor
  `.with_node_dispatch(WarmNodeDispatch{sm, ctx})` for a `--context` workflow run.
- Drain-on-cancel (W3b) is PRESERVED: the executor's `FuturesUnordered` + cancel token logic is untouched; the
  only change is how (backend, session) is acquired + cleaned up per node. The prompt+drain loop
  (`executor.rs:131-152`) is shared.

## 4. forget→finish per-site (Q4)

The cold path keeps `forget_session` at all sites (`:115/:122/:127/:153`). The warm path's `NodeCleanup`:
- **Normal exit (`:153`)** → `finish_turn`→Idle (keep warm). The session is reused next run.
- **Cancel mid-turn (`:137`)** → `backend.cancel(&session)` then `finish_turn`→Idle (mirror the single-agent
  warm cancel: the SM keeps the session warm; W3b drain intact). Do NOT forget.
- **Cancel/err before/in prompt (`:115/:122/:127`)** → `finish_turn`→Idle (the warm session was checked out;
  return it to Idle so the next run reuses or reconciles it). A configure/prompt error leaves the warm session
  Idle (not forgotten) — the next checkout reconciles or the operator releases.
- All cleanup is via the `NodeCleanup` the warm dispatch returned, so the executor stays backend-agnostic.

## 5. Per-node warm-session cleanup / lifecycle (Q5)

`wf-{C}-{node}` sub-contexts are independent warm handles. Cleanup story for the MVP:
- **TTL reap** (the shipped reaper) frees them after the workflow goes idle past `warm_idle_ttl` — the default,
  zero new code.
- **`session release C`** should ALSO free the workflow's children. RECOMMEND a **prefix-release**: `release`
  (and the operator-facing `SessionRelease`) on a context releases that context AND any `wf-{C}-*` children
  (iterate `by_context` for the `wf-{C}-` prefix). Small SM addition; gives the operator a single "free this
  workflow's warm agents" lever.
- **`clear`/`compact` on a workflow context = N/A for the MVP** (those are single-session ops; a workflow
  context is a namespace, not one session). Document; defer per-node clear/compact.
- OPEN: whether the parent `C` itself is ever a warm handle (it is NOT in this design — only `wf-C-*` are) →
  `status C` for a workflow context returns the children's aggregate or N/A (spec decides; lean: a new
  `SessionStatus` on `wf-C-node` works per-node, and `C` alone is a namespace).

## 6. gate() rejection lift (Q6)

Remove the Slice-0 "contextId only on the Local route" rejection (`server.rs:352`) FOR THE WORKFLOW ROUTE.
Safety: the per-node warm sessions (`wf-C-node`) are independent of the workflow TASK's SEQ-AUTHORITY (the task
is keyed by taskId, the warm sessions by the derived contexts). **MVP = streaming path only** (`spawn_workflow_
producer`); the detached/durable (W3a/W3b) warm path is deferred (it adds the task-vs-handle SEQ-AUTHORITY
question on context C). So: allow contextId on the Workflow route for the STREAMING send; keep detached
workflow submit contextId-less for now (document).

## 7. CLI client + flags (Q7)

- `run-workflow <wf> --input <f> [--session-cwd R]` — UNCHANGED local one-shot (back-compat).
- `run-workflow --serve [--url U] --context C <wf> --input <f> [--session-cwd R]` — client: POST a STREAMING
  workflow message (skill=`<wf>`, `message.contextId=C`, parts=input, metadata cwd) to U (default
  `http://127.0.0.1:8080`); stream SSE events to stdout (reuse the `collect_sse`/`task watch` client shape).
- **`--context` implies `--serve`** (a contextId is meaningless to a local one-shot) — either auto-imply or
  error if `--context` without `--serve`. Lean: `--context` REQUIRES `--serve` (explicit; error otherwise).
- **`--config` is LOCAL-ONLY**: for `--serve`, the workflow definition lives in the SERVE's config (the serve
  was started `serve --config <with-workflows>`); the client just names the workflow-id. Reject `--config`
  with `--serve` (or ignore + warn). The serve owns its workflows.
- Streaming (not unary): workflows emit many events; reuse the SSE client.

## 8. DoD / live-gate (Q8)

1. `serve --config <wf-config>` (a multi-node workflow, e.g. a 2-node `design` or `code-review`, codex/claude,
   large `warm_idle_ttl`).
2. `run-workflow --serve --url U --context C <wf> --input <f>` (run #1) — agents cold-spawn (~27s/agent);
   record the agent pids (serve's children).
3. `run-workflow --serve --url U --context C <wf> --input <f>` (run #2) — **no cold spawn**: SAME agent pids,
   sub-second agent start, faster wall-clock. (The felt win.)
4. `session release C` frees the workflow's warm agents (pids gone). [prefix-release]
5. **Back-compat:** `run-workflow <wf> --input <f>` (no `--serve`) runs locally + unchanged.

## 9. Recommended architecture (the build)
- A `NodeDispatch` trait (dependency-inversion seam) in `bridge-workflow`/`bridge-core`; `WorkflowExecutor`
  gains `Option<Arc<dyn NodeDispatch>>` (builder); `None` = today's cold path BYTE-IDENTICAL (back-compat).
- A `WarmNodeDispatch{sm, ctx}` impl in `bridge-a2a-inbound`: derives `wf-{C}-{node_id}`, `checkout_turn` +
  `finish_turn` (no forget), mirroring `WarmTurnGuard`.
- The serve wires `.with_node_dispatch(WarmNodeDispatch)` when a Workflow message carries a contextId; gate()
  lifts the contextId-on-Workflow rejection (streaming path only).
- `release` becomes prefix-aware (`release C` frees `wf-C-*`); TTL reap as the backstop.
- CLI: `--serve [--url] --context` client (streaming SSE POST); `--context` requires `--serve`; `--config`
  local-only.

## 10. Top risks (ranked)
1. **The `NodeDispatch` seam refactor of `run_node`** — extracting acquire/cleanup without changing the cold
   path's behavior or the drain-on-cancel semantics. Mitigate: keep the cold path inline, add the warm path
   behind `Option`; share only the prompt+drain loop.
2. **Per-node warm cleanup leak** — `wf-C-*` children outliving `C` (mitigated by prefix-release + TTL, but the
   prefix-release is a new SM behavior to get right).
3. **Drain-on-cancel under warm dispatch** — a cancelled warm node must `backend.cancel`+`finish` (not forget),
   and the executor's `FuturesUnordered` drain must stay intact (W3b).
4. **CLI streaming client** — correctly POSTing a workflow streaming message + consuming SSE (the wire shape
   for a workflow skill + contextId).

## 11. Open questions for the spec
- O1: the exact derived-context format + `ContextId` charset validation (`wf-{C}-{node}` vs a separator the
  newtype accepts).
- O2: `NodeDispatch` location (`bridge-workflow` vs `bridge-core::ports`) + the exact `NodeLease`/`NodeCleanup`
  shape (boxed closure vs enum).
- O3: prefix-release semantics (does `release C` need C to be a live handle, or is it a pure prefix sweep?).
- O4: streaming-only MVP vs also supporting detached `--serve` (SEQ-AUTHORITY on C) — lean streaming-only.
- O5: `--context` requires-`--serve` (error) vs auto-implies — lean require (explicit).
- O6: does `--serve` reuse the executor's existing `run_with_context` (just threading the dispatch), confirming
  the FuturesUnordered/drain code is 100% untouched?

— END Opus lens. codex xhigh lens fold below. —

## codex gpt-5.5 xhigh fold + convergence (2026-06-19)

Codex ran an independent read-only pass (`/tmp/slice5-arch-codex.out`). **CONFIDENCE: high.** CONVERGED with
the Opus lens on: `--serve` client via `SendStreamingMessage`; derived per-node child contexts (Option A) not
SM-rekey/single-agent; the bridge-workflow-owned dispatch trait (dependency inversion, cold in workflow / warm
in inbound); forget→finish per-site; gate lift for the Workflow route only; `--context` REQUIRES `--serve`;
`--config` serve-side; streaming-only MVP; drain-on-cancel preserved (executor's `FuturesUnordered`,
`executor.rs:322`, untouched by the new dispatcher-aware methods).

**Refinements ADOPTED from codex (override the Opus draft where noted):**
1. **Parent-child TRACKING, NOT prefix-release (KEY correction).** `ContextId` accepts ANY non-empty string
   (`ids.rs:10/34`), so a `wf-{C}-` prefix sweep is UNSAFE (collisions). Instead: the warm checkout REGISTERS
   `parent C → child_context` (a `parent→Vec<child>` map on/beside `SessionManager`); `SessionRelease C` /
   `SessionClear C` / `SessionCancel C` operate on exact `C` PLUS its registered children. TTL = backstop, not
   primary. (Supersedes §5's prefix-release.)
2. **Child context format `<parent>::workflow::<wf_id>::node::<node_id>`** (per-NODE, not per-agent — two
   claude nodes have different histories; includes `wf_id` so the same parent context can host different
   workflows). `::` is charset-safe (ContextId is unvalidated). (Refines §2's `wf-{C}-{node}`.)
3. **Seam = NEW dispatcher-aware executor methods** (`run_with_context_and_dispatcher` /
   `run_from_with_context_and_dispatcher`); the EXISTING methods delegate to a `ColdWorkflowNodeDispatcher`
   (resolve+configure+forget). Trait `WorkflowNodeDispatcher::checkout(wf_id,node,run_id,op,ctx) -> NodeTurn{
   backend,session,seed,cleanup: Box<dyn NodeTurnCleanup>}`. **OPEN (for the spec/plan-review):** unifying the
   cold path into a `ColdDispatcher` is cleaner but refactors the byte-identical cold path → MUST be guarded by
   a back-compat test, else keep cold inline + add the warm path. (Refines §3.)
4. **Per-site forget→finish (codex's precise map):** `:115`/`:122` → `finish_turn` (no backend cancel, no
   stream started); `:127` prompt-error → `finish_turn`, keep the handle UNLESS the error class is unrecoverable
   (→ expire); `:137` stream-cancel → `SessionManager::cancel(child)` (warm cancel) then guarded finish; `:148`
   stream-err → `finish_turn` (matches `WarmTurnGuard` `server.rs:514`); `:153` normal → cold forgets / warm
   finishes. (Refines §4 — adds the `:148` split + the unrecoverable-error classification.)
5. **Seed injection:** the warm node dispatch must PREPEND `NodeTurn.seed` (the `pending_seed` from a prior
   Slice-4 `compact` on that child context) to the node's prompt parts — the Slice-4 integration point.
6. **Concurrent same-context workflow runs → `HandleBusy` early** (don't let node-level failure be the only
   signal); the child checkouts would otherwise collide.
7. **Live-gate observes warmth structurally** (codex): assert child-context checkout = miss/mint on run #1,
   HIT (same `backend_session`) on run #2, AND no 2nd backend spawn (registry spawn-once,
   `registry.rs:305/817`) — more robust than wall-clock timing.

**Open questions carried to the spec:** the cold-path refactor approach (ColdDispatcher vs inline — back-compat
keystone); prompt-error finish-vs-expire classes; `SessionCompact C` on a workflow context (all children vs
N/A — lean N/A for MVP); whether warm dispatch honors model/effort overrides (executor passes `None` today —
lean keep `None`, defer). **Net: strong convergence, codex's parent-child-tracking + the per-site map are clean
upgrades. Ready to write the spec.** → `docs/superpowers/specs/2026-06-19-slice-5-serve-cli.md`.
