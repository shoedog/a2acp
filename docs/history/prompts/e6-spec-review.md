You are doing a rigorous, adversarial SPEC REVIEW (read-only) of "E6 — Node Retry (transient-failure retry/resume)"
for the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). READ-ONLY: read the spec + the real
code; do NOT edit/build/test.

The spec: `docs/superpowers/specs/2026-06-24-e6-node-retry.md`. E6 lets a workflow node that fails with a TRANSIENT
agent error (crash/overload/watchdog-timeout/startup-flake) auto-retry within the run (bounded attempts + backoff)
before degrading to `ok=false`. Opt-in per node; default off (zero behavior change).

Binding context — VERIFY every anchor against the real code:
- `crates/bridge-workflow/src/executor.rs` — `run_node` (`:158`): the retry hook. The failure sites are the
  prompt/prompt_observed error (`~:318-330`) and the drain-loop `Some(Err(e))` (`~:355`); cancellation arms at
  `:301`/`:340`; `STOP_REASON_CANCELLED` at `:352`; `forget_session` on every exit; the 3-tuple return
  `(String, bool, Option<UsageSnapshot>)` (`:167`). The cold path already FAILS the node on `configure_session`
  error (`:275`, the E1/SR-FIX-1 fix).
- `crates/bridge-core/src/error.rs` — `BridgeError` variants (`:22-72`) + `is_resumable()` (`:127`) +
  `disposition()` (`:107`). The spec adds `is_transient()` here. VERIFY the proposed transient set
  (`AgentCrashed | AgentOverloaded | AgentTimedOut`) vs every variant.
- `crates/bridge-workflow/src/graph.rs` — `WorkflowNode { id, agent, prompt_template, inputs }` (`:20`) +
  `WorkflowGraph.panel: Option<PanelConfig>` (`:16`, the additive-spec-snapshot precedent the spec mirrors for
  `retry`). The durable spec snapshot = `encode_workflow_spec` (Slice-10 made `panel` ride it resume-free).
- `bin/a2a-bridge/src/config.rs` — `WorkflowNodeToml { id, agent, prompt_file, inputs }` (the `retry` add-point).
- W3b crash-resume: `crates/bridge-coordinator/src/detached.rs` `resume_working_tasks` (the seed is built from
  `node_checkpoints` = `(node, output, ok, usage)` INCLUDING `ok=false`; the `resume_attempts` cap bounds loops). The
  spec's resume-compatibility claim ("a mid-retry unfinished node has no checkpoint → re-runs free") + the deferral
  ("don't re-run EXHAUSTED `ok=false` checkpoints") hinge on this — VERIFY.
- The E9 watchdog (`AgentTimedOut`) + Slice-10 usage carrier interact with retry (spec Q6/Q7).

{{input}}

GROUND every finding in a real `file:line`. Pressure-test:
1. **The retry hook correctness.** Can the configure→prompt→drain core actually be wrapped in a bounded loop in
   `run_node` WITHOUT breaking cancellation (the `biased` selects), the rich-sink flush, or `forget_session`
   discipline? Is `forget_session`-then-re-`configure_session` between attempts correct for the cold backend (does a
   re-configure after forget re-establish a clean session)? Any state that leaks across attempts (rich sink, partial
   `text`, usage)?
2. **The transient taxonomy (Q4).** Is `AgentCrashed | AgentOverloaded | AgentTimedOut` the right set? Should
   `FrameError` (protocol desync) / `UpstreamA2aError` / `StoreFailure` be in or out? Is retrying `AgentTimedOut`
   sound (the watchdog tripped — will a retry just trip again, or is a transient hang exactly the case retry helps)?
   Is `CancelTimeout`/cancel correctly EXCLUDED (never retry user intent)? Walk EVERY variant.
3. **Cancellation + backoff.** Does a cancel mid-backoff abort promptly (the sleep MUST be `select!`-able against the
   cancel token)? Does a cancel during a retry attempt behave like today (canceled marker, no further attempts)? Any
   way the retry loop swallows a cancel or relabels it?
4. **Resume-compatibility + the deferral.** Is the claim "a mid-retry node is unfinished → not checkpointed → re-runs
   free on W3b resume" actually TRUE against `detached.rs`/`run_from`? Is the deferral (don't re-run EXHAUSTED
   `ok=false` checkpoints) the right cut, or does it leave a real gap (a transient failure that exhausted in-run
   retries then a restart could have succeeded)? Is the `resume_attempts` cap still the poison backstop?
5. **Plumbing + durability.** Does `WorkflowNode.retry: Option<RetryPolicy>` ride `encode_workflow_spec` resume-safe
   like `panel` (additive, `skip_serializing_if`)? Any place a node is reconstructed that would DROP `retry`
   (resume, the W3b spec snapshot, the panel/costs paths)? Does `WorkflowNodeToml.retry` → `WorkflowNode.retry`
   mapping have a clean home (`into_snapshot`/graph build)?
6. **Usage accounting (Q7/D5).** On a retried node, is summing usage across attempts correct + implementable given
   `last_usage` is last-delta-only per attempt? Or is last-attempt-only simpler + acceptable? Any double-count risk
   vs the Slice-10 carrier?
7. **Watchdog interaction (Q6).** Does the E9 per-turn watchdog (`AgentTimedOut` via the `biased` outer select)
   compose with the retry loop — does the watchdog fire per ATTEMPT (good) or get confused across attempts? Any
   double-fire / relabel hazard (the Slice-7b BLOCKER was an unbiased select relabeling a completed turn)?
8. **Scope + missing pieces.** Is the MVP cut (per-node, default-off, tracing-observability, defer
   resume-re-run-of-failures + warm-turn-retry) right? Any Q (Q1–Q7) the spec leaves dangling that MUST be decided
   before planning? Any wrong `file:line`. Anything the spec must add or cut. Any spike needed (e.g. does a
   forced-flaky `AgentBackend` cleanly drive `run_node` retries)?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. Answer
Q1–Q7 + D1–D6. End with `SPEC VERDICT: ready-to-plan | needs-revision | needs-spike`.
