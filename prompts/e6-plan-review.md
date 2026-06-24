You are doing a rigorous, adversarial PLAN REVIEW (read-only) of the implementation plan for "E6 — Node Retry" for
the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). READ-ONLY: read the plan + the binding
spec + the real code; do NOT edit/build/test.

- PLAN: `docs/superpowers/plans/2026-06-24-e6-node-retry.md` (6 TDD tasks T1–T6).
- BINDING SPEC: `docs/superpowers/specs/2026-06-24-e6-node-retry.md` — the `## v2` (SR-FIX-1..6) + `## v3`
  (RR-FIX-1..4) sections supersede the v1 body.
- The plan claims specific `file:line` anchors + exact type/signature changes. VERIFY each against the real code.

E6 = opt-in per-node retry of TRANSIENT agent failures in the COLD workflow executor (`run_node`), default off. The
reset between attempts is `release_session` + a NEW `AgentRegistry::invalidate(agent)` (atomic Slot replace + retire
old → next `resolve` RESPAWNS a fresh process). Recovers `AgentCrashed` (startup + mid-turn) + `AgentTimedOut`
(killed-or-not) + `AgentOverloaded`.

Key code to verify the plan against:
- `crates/bridge-core/src/error.rs` — `BridgeError` variants (`:22-72`), `is_resumable()` (`:127`). T1's transient
  set = `AgentCrashed | AgentOverloaded | AgentTimedOut`.
- `crates/bridge-workflow/src/graph.rs` — `WorkflowNode { id, agent, prompt_template, inputs }` (`:20`),
  `WorkflowGraph.panel` (`:16`, the additive-snapshot precedent). EVERY `WorkflowNode { .. }` literal that T2's new
  field forces to update.
- `bin/a2a-bridge/src/config.rs` — `WorkflowNodeToml` + the graph-build mapping to `WorkflowNode` (T3).
- `crates/bridge-core/src/ports.rs:197-213` — the `AgentRegistry` trait (T4 adds `invalidate`). Every `impl
  AgentRegistry` (real + mocked/test) that the default-no-op must keep compiling.
- `crates/bridge-registry/src/registry.rs` — `Slot { entry: ArcSwap<AgentEntry>, backend: OnceCell<...> }` (`:32`),
  `State { slots, ... }` in `state: ArcSwap<State>` (`:93`), `resolve` lazy-spawn (`:308`), `apply` atomic swap
  (`:366-412`), `retire()` (`:486`). T4's `invalidate` mirrors `apply`'s swap + retires the old backend.
- `crates/bridge-workflow/src/executor.rs` — `run_node` (`:158-388`): the resolve cancel-select (`:266`), configure
  fail-on-error (`:275`, T6 regression test `:1549`), prompt/drain sites (`:299/337/355`), cancel arms
  (`:301/340/352`), rich-flush + `forget_session` (`:363-388`), 3-tuple return (`:167`); the existing cold test
  harness (`~:1480/1519/1549`) + `Rec` (`:646`). Is `self.registry` reachable here + `Arc<dyn AgentRegistry>`?
- `crates/bridge-acp/src/acp_backend.rs` — `release_session` (`:2705`, bridge-side, process-preserving), watchdog
  process-kill on `AgentTimedOut` (`:2376/2435`), `retire()`.

{{input}}

GROUND every finding in real `file:line`. Pressure-test the PLAN specifically:

1. **Compile-green per task.** Trace the type ripples. T2 adds `WorkflowNode.retry` — does the plan update EVERY
   `WorkflowNode { .. }` literal (construction + tests) so the crate compiles? T4 adds a trait method — does the
   default no-op keep ALL `impl AgentRegistry` (real + every mock in tests across crates) compiling? Does T4's
   `invalidate` body match the REAL `State`/`Slot` shape (the plan sketches `State { slots, default }` — verify the
   actual fields)? Does T3's mapping site exist where the plan says? Flag any task that would NOT compile.

2. **The `invalidate` seam (T4 — the riskiest).** Is "atomic Slot replace via `state.store` + retire the old
   backend" correct + race-safe? Does retiring the old backend BLOCK (does `retire()` drain leases / wait — would
   that stall the retry path)? Should it be best-effort/non-blocking/spawned? Is the apply-vs-invalidate `ArcSwap`
   race (a concurrent `apply` clobbering the invalidate, or vice-versa) acceptable, or does it need an rcu/CAS? Does
   replacing the Slot ORPHAN a still-resolving caller (a concurrent `resolve` mid-`get_or_try_init`)? Does retiring a
   backend that OTHER concurrent sessions still hold (their `Arc` clone) break them?

3. **The retry loop (T5 — the core).** Is the resolve-INSIDE-the-loop restructure faithful to the existing
   cancel-selects, the rich-sink flush, and the `forget_session` discipline? Does the plan correctly classify each
   site's error as Transient/Fatal via `is_transient()` and PRESERVE the T6 configure fail-fast (`ConfigInvalid` ⇒
   Fatal ⇒ no retry; regression test `:1549` stays green)? Is the reset `release_session` + `invalidate` correct, or
   is `release` redundant/harmful given the respawn (double-cancel? releasing a session on a backend about to be
   invalidated)? Is the backoff `tokio::select!`-cancel-abortable AND overflow-safe (T2 `backoff_for`)? Does
   `tokio::time` need a feature add to `bridge-workflow`? Does last-attempt usage thread correctly?

4. **Ordering + seam correctness.** Is the bottom-up T1→T6 order right (no forward references)? Does T5 depend on T4's
   `invalidate` + T2's `RetryPolicy` + T1's `is_transient` (all earlier)? Does the loop re-bind `resolved` each
   iteration so an invalidated agent actually respawns?

5. **Test quality.** Are the TDD tests real failing-first? Does T5's `FlakyBackend` + invalidate-counter harness
   actually exercise the loop (fail-then-ok, exhaust, non-transient, no-policy, cancel-mid-backoff)? Can a test count
   `invalidate` calls (does it need a wrapping/fake `AgentRegistry`)? Does T6 actually prove "no checkpoint on a
   mid-retry interrupt"? Any test that passes even if retry is broken?

6. **Faithfulness to spec v3 + missing pieces.** Each SR-FIX/RR-FIX → a task? (is_transient D4=T1; RetryPolicy+backoff
   Q1/D1=T2; config=T3; invalidate RR-FIX-1=T4; loop SR-FIX-1/2/3/4/6 + RR-FIX-2=T5; resume-compat=T6.) Any wrong
   `file:line`. Any step with a placeholder/undefined type. Is the per-task granularity right? Anything the plan must
   add or cut (e.g. should `invalidate`'s retire be its own task; is the respawn-every-attempt vs respawn-on-death
   decision settled; is `self.registry` access a hidden prerequisite)?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. End
with `PLAN VERDICT: ready-to-implement | needs-revision`.
