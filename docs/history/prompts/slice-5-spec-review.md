You are reviewing the SPEC (design, not the plan) for Slice 5 (Serve-backed `run-workflow` + handle-aware
keep-warm) of a2a-bridge — the MVP cut-line. session-cwd = the bridge repo. READ-ONLY: read files, grep, `git`;
do NOT edit/build/test. Judge whether this spec, if planned + implemented faithfully, produces correct,
compiling, spec-faithful code that meets the DoD, and whether the design is sound. The spec came from a
dual-lens analysis (Opus + codex-xhigh CONVERGED) — STRESS it, don't rubber-stamp. Severity-tag
BLOCKER / MAJOR / MINOR with concrete fixes (file:line).

Slice 5 = `run-workflow --serve --context C <wf>` makes the CLI a serve client so a workflow's agent sessions
stay WARM in the serve and REUSE across invocations (no cold respawn on the 2nd run); handle-aware executor
keep-warm (no per-node `forget_session`); drain-on-cancel preserved; non-serve path byte-identical. The spec is
below.

{{input}}

READ FOR GROUND TRUTH (cite file:line):
- The KEYSTONE — crate layering: `crates/bridge-workflow/src/executor.rs` (`WorkflowExecutor{registry}` `:30`;
  `run_node` `:67`, session-mint `:80`, resolve `:98`, configure `:104`, prompt `:125`, drain `:131-152`,
  forget sites `:115/:122/:127/:153`, cancel `:137`; the `FuturesUnordered` scheduler + drain-on-cancel `:322`;
  entry pts `:159/:171/:185`). Confirm `bridge-workflow` does NOT (and must not) depend on `bridge-a2a-inbound`
  (check `crates/bridge-workflow/Cargo.toml`) — the dispatch trait must respect this inversion.
- The single-agent WARM pattern to mirror: `crates/bridge-a2a-inbound/src/server.rs` `warm_local_dispatch`
  (`:614`), `WarmTurnGuard` (`:506`, Drop→`finish_turn`); `spawn_workflow_producer` (`:1683`); `gate()` +
  the contextId-on-non-Local rejection (`:352`); `context_id_from_params` (`:3286`).
- `crates/bridge-a2a-inbound/src/session_manager.rs`: `SessionManager` (`:114`), `WarmHandle` (`:39`, ONE
  agent/`backend_session`; `ConfigMismatch{agent}` on agent change `:231`), `checkout_turn` (`:191`, reuse
  `:216`, mint `:322`), `finish_turn` (`:367`), `cancel` (`:440`), `release` (`:421`), `reset_session`,
  `compact_session`, `reap_idle` (`:702`). Confirm the warm child-context + parent→child tracking compose.
- `crates/bridge-core/src/ids.rs` `ContextId` (`:10/:34` — accepts ANY non-empty string → why prefix-matching
  is unsafe + parent-child tracking is required). `crates/bridge-core/src/domain.rs` `RouteTarget::Workflow`
  (`:226`); `bin/a2a-bridge/src/route.rs` skill→workflow routing (`:41`).
- CLI: `bin/a2a-bridge/src/main.rs` `run_workflow_cmd` (`:2233`), `parse_run_workflow_args` (`:543`), the local
  exec (`:2352-2396`), `rpc_call` (`:2571`), `submit_cmd` contextId (`:2637`), `task watch`/SSE (`:2753`).

REVIEW DIMENSIONS (ground each in code):
1. **Layering / the dispatch seam (the keystone).** Does `WorkflowNodeDispatcher` (defined in `bridge-workflow`,
   warm impl in `bridge-a2a-inbound`) correctly invert the dependency (no `bridge-workflow → bridge-a2a-inbound`
   edge)? Are the trait + `NodeTurn`/`NodeTurnCleanup` types expressible with only `bridge-core` types? Does the
   `run_node` refactor preserve the `FuturesUnordered` drain-on-cancel (`:322`) and the shared prompt+drain
   loop? Will it COMPILE (async-trait, `Box<dyn NodeTurnCleanup>` `Send`, the `self: Box<Self>` cleanup)?
2. **Back-compat (DoD-critical).** Does routing the COLD path through a `ColdWorkflowNodeDispatcher` keep the
   node behavior byte-identical (same `workflow-{wf}-{node}-{run_id}` session id + forget at every site)? Is the
   `cold_dispatcher_matches_legacy_behavior` test sufficient, or is keeping the cold path inline safer? Does the
   local `run_workflow_cmd` stay unchanged?
3. **Per-node child keying.** Is `<parent>::workflow::<wf_id>::node::<node_id>` collision-safe + a valid
   `ContextId`? Per-node (not per-agent) correct (two claude nodes)? Does the existing `checkout_turn` warm a
   derived child cleanly (the `ConfigMismatch{agent}` guard is per-context, so each child has ONE agent —
   confirm a child context is only ever used by its one node's agent)?
4. **Parent→child tracking + lifecycle.** Is tracking (not prefix) correctly specified? Where does the map live
   + who registers (checkout_turn vs the warm dispatcher)? Do `release/clear/cancel C` sweep children
   correctly + atomically (no race vs an in-flight run)? Is `compact/status C` on a parent = N/A acceptable?
5. **forget→finish per-site map.** Is the §5 table correct vs the actual `executor.rs` sites + the single-agent
   warm semantics? The `:127` finish-vs-expire classification + `:137` warm-cancel — sound? Drain-on-cancel
   intact?
6. **Seed injection** (Slice-4): is prepending `NodeTurn.seed` to the node prompt parts correct + only on warm?
7. **gate lift + concurrency.** Is lifting contextId for `Workflow(_)` only (Delegate/Fanout still rejected)
   safe (parent never a warm handle)? Is the concurrent-same-context `HandleBusy` correctly placed?
8. **CLI client.** `--context` requires `--serve`; `--config` rejected with `--serve`; the
   `SendStreamingMessage` wire shape (contextId + `a2a-bridge.skill` + parts + cwd); SSE consumption + exit
   code. Right vs the actual wire (`submit_cmd`/`task watch`)?
9. **Scope.** Streaming-only MVP (detached/W3a deferred), no overrides, compact-C N/A — correctly cut? Any gap
   vs the DoD or scope creep?
10. **TDD realizability + O1–O5.** Are the listed tests writable on the existing harness? Recommend on O1
    (cold-path refactor), O2 (error classes), O3 (cleanup shape), O4 (map location), O5 (streaming vs detached).

OUTPUT: findings by severity (file:line, concrete fix); spec-faithfulness verdict; design-soundness verdict;
recommendations for O1–O5. End with exactly: `SPEC VERDICT: ready-to-plan | fix-then-plan | rework`.
