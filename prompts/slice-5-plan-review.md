You are reviewing an IMPLEMENTATION PLAN (not the design) for Slice 5 (Serve-backed `run-workflow` +
handle-aware keep-warm) of a2a-bridge — the MVP cut-line. session-cwd = the bridge repo. READ-ONLY: read files,
grep, `git`; do NOT edit/build/test. Judge whether a codex implementor following this plan task-by-task
produces correct, COMPILING, spec-faithful code that passes each task's tests AND `cargo test --workspace
--no-run` at each boundary. Severity-tag BLOCKER / MAJOR / MINOR with concrete fixes (task #, file:line).

The DESIGN is frozen: spec `docs/superpowers/specs/2026-06-19-slice-5-serve-cli.md` v2, **FIX-1..11 BINDING**
(the "## v2" section). Slice 5 = `run-workflow --serve --context C <wf>` makes the CLI a serve client so a
workflow's per-node agent sessions stay warm + reuse across runs; handle-aware executor keep-warm (no per-node
forget); non-serve path byte-identical. KEYSTONE: crate layering — the executor (`bridge-workflow`) must NOT
import `SessionManager` (`bridge-a2a-inbound`); the seam is a dependency-inversion trait. The plan is below.

{{input}}

READ FOR GROUND TRUTH (cite file:line):
- The v2 spec (esp. "## v2 — FIX-1..11").
- `crates/bridge-workflow/src/executor.rs` — `WorkflowExecutor{registry}` (:30), `run_node` (:67; session-mint
  :80; resolve :98; configure :104; prompt :125; drain :131-152; forget :115/:122/:127/:153; cancel :137);
  scheduler+drain (:322); entry pts (:159/:171/:185). `crates/bridge-workflow/Cargo.toml` (`async-trait`
  dev-only :16).
- `crates/bridge-a2a-inbound/src/session_manager.rs` — `SessionManager` (:114), `WarmHandle` (:39, ONE agent;
  `ConfigMismatch{agent}` :231), `WarmTurn` (:60, carries generation+op+seed), `checkout_turn` (:191, reuse
  :216, mint :322), `finish_turn` (:367, no-ops unless gen+op+Running), `cancel` (:440), `release` (:421),
  `reset_session`, `reap_idle` (:702).
- `crates/bridge-a2a-inbound/src/server.rs` — `gate()` + contextId rejection (:352), `context_id_from_params`
  (:3286), `warm_local_dispatch` (:614), `WarmTurnGuard` (:506), `spawn_workflow_producer` (:1683),
  `workflow_cancels` (:1711/:1731), `session_cancel`/`session_release`/`session_clear` handlers (:3104/:3005/
  :3030), the unary vs streaming split (:2367/:829).
- `crates/bridge-core/src/ids.rs` `ContextId` (:10/:34), `crates/bridge-core/src/error.rs` `AgentCrashed` (:57).
- `bin/a2a-bridge/src/main.rs` — `run_workflow_cmd` (:2233), `parse_run_workflow_args` (:543, the CONFIG_PATH
  default :593), local exec (:2352-2396), `rpc_call` (:2571), `submit_cmd` (:2598/:2616/:2637), `task_watch_cmd`
  SSE (:2756).

REVIEW DIMENSIONS (ground each in code):
1. **Spec faithfulness.** Does each FIX-1..11 map to a task STEP that implements it? Any gap or scope creep?
2. **Task ordering / compile-at-each-boundary.** T1 (trait + async-trait dep) before T2 (uses the trait)? Does
   T2's `Option<dispatcher>` + the warm branch keep the cold path byte-identical (back-compat)? Do T3/T4
   (SessionManager) compile before T5 (uses `checkout_child_turn`)? Does each task pass `--no-run` at its end?
3. **T2 — the executor warm branch (highest risk).** Is the cold path GENUINELY untouched (the `None` branch)?
   Is the seed-prepend correct + warm-only? Is the SHARED prompt+drain reused without breaking the
   `FuturesUnordered` drain-on-cancel (:322)? Does `run_node` returning a node-error marker on `checkout` Err
   (not `?`/panic) match the cold path's error convention? Will the `Option<Arc<dyn WorkflowNodeDispatcher>>` +
   `Box<dyn NodeTurnCleanup>` COMPILE (Send bounds, `self: Box<Self>`, async-trait)?
4. **T3 — `checkout_child_turn` atomicity.** Does registering `parent→child` AFTER `checkout_turn` success
   (FIX-2/M3) avoid the stale-on-failure leak? Does the returned `WarmTurn` carry the exact gen+op the warm
   cleanup needs (else `finish_turn` no-ops → child stranded → DoD defeated)? Is the derived child a valid
   `ContextId` + does the per-context `ConfigMismatch{agent}` guard stay correct (one agent per child)?
5. **T4 — the sweep helpers.** Do `{release,clear,cancel}_with_children` snapshot under the lock + tolerate an
   absent parent (success, not `SessionNotFound`)? Any deadlock (taking `children` lock + `by_context` lock)?
6. **T5 — `WarmWorkflowNodeDispatcher`.** Does `on_exit` map correctly (Normal/Error(other)→finish;
   Canceled→`sm.cancel`; Error(AgentCrashed)→`expire_turn`)? Does the cleanup close over gen+op (not borrow)?
7. **T6 — the run guard + scheduler-cancel.** Is the concurrent `HandleBusy` returned EARLY (before SSE
   commit)? Does `SessionCancel C` cancel the run token (stopping the scheduler) THEN sweep children? Is the
   guard removed on producer exit (no leak, mirror `workflow_cancels`)?
8. **T7 — gate + unary reject + sweep handlers.** Is the gate match arm correct (Workflow allowed, Delegate/
   Fanout rejected)? Is the unary-workflow-contextId rejection placed right? Do the release/clear handlers
   sweep + treat absent-parent as success?
9. **T8 — CLI.** `--context` requires `--serve`; `--config`+`--serve` detected on the un-defaulted Option; the
   `SendStreamingMessage` map built directly (not via `submit_cmd`'s skill-guesser); SSE consumed + exit code.
10. **TDD realizability.** Are the FakeDispatcher / recording-backend / warm-test-harness / CLI tests writable
    on the EXISTING harness (e.g. the executor's `cancel_drains_inflight` backend; the server warm harness)?
    Any test not writable as described?
11. **Live-gate provability** (T9).

OUTPUT: findings by severity (task #, file:line, fix); spec-faithfulness verdict; task-ordering verdict;
code-correctness verdict. End with exactly: `PLAN VERDICT: ready-to-execute | fix-then-execute | rework`.
