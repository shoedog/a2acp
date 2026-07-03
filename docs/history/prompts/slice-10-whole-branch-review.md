You are doing a rigorous, adversarial WHOLE-BRANCH REVIEW (read-only) of the COMPLETE implementation of
"Slice 10 — B2: Weighted Fan-out Panel" for the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow
orchestrator). READ-ONLY: read the diff + the real code; do NOT edit/build/test.

The branch is `feat/slice-10-fanout-panel`. Inspect the FULL diff vs main:
  `git diff main...HEAD`
(13 commits, T1–T8 + docs/scaffolding). Binding spec: `docs/superpowers/specs/2026-06-22-slice-10-fanout-panel.md`
(`## v2`, SF-FIX-1..6). Binding plan: `docs/superpowers/plans/2026-06-22-slice-10-fanout-panel.md` (`## v2`,
PR-FIX-1..10). The per-task tests pass and the full workspace gate is green (1209 passed); your job is to find what
the per-task tests + the happy-path gate MISSED — correctness, races, durability holes, contract violations.

What B2 does: per-node usage is captured in the executor, threaded durably through the journal +
`task_node_checkpoints.usage_json` column + the crash-resume seed, and surfaced via two reserved synth template
vars `{{workflow.costs}}` (from captured usage) and `{{workflow.weights}}` (from `[workflows.panel]` config). A
`panel` workflow instantiates fan-out→weighted-synth.

Key files: `crates/bridge-workflow/src/executor.rs` (capture in both drain loops, NodeFut/outputs 3-tuple, fan-in
injection, render_costs_table/render_weights, run_from seed type), `crates/bridge-workflow/src/graph.rs`
(WorkflowGraph.panel), `crates/bridge-core/src/orch.rs` (NodeFinished.usage), `crates/bridge-core/src/task_store.rs`
(trait + MemoryTaskStore + fold), `crates/bridge-store/src/sqlite.rs` (usage_json column + migration + read/write),
`crates/bridge-coordinator/src/detached.rs` (WorkflowSink/drain/DetachedProgressSink + FrameKind.usage +
resume_working_tasks seed), `crates/bridge-a2a-inbound/src/server.rs` (SseSink + snapshot-replay frame + forwarding
wrapper), `bin/a2a-bridge/src/config.rs` ([workflows.panel] parse).

{{input}}

GROUND every finding in real `file:line`. Pressure-test ACROSS the whole change:

1. **Usage capture correctness.** "Last Update::Usage before Done wins" — is that right for cumulative usage in
   BOTH the warm/dispatcher loop (~executor.rs:169) and the cold loop (~:275)? On cancel/error, is `usage: None`
   correct, or is a partial usage lost that should be kept? Does a node that emits NO usage (api backend) correctly
   yield None → `n/a`?

2. **Durability — the SF-FIX-4 chain end-to-end.** Trace capture → `WorkflowEvent::NodeFinished.usage` →
   `DetachedProgressSink` → `put_node_checkpoint_sequenced(usage)` → both the `usage_json` column AND the journal
   `NodeFinished{usage}` → on restart `resume_working_tasks` reads `node_checkpoints` (4-tuple) → seed →
   `run_from` → fan-in `{{workflow.costs}}`. Is it ACTUALLY closed with no drop? Is the `ALTER TABLE ADD COLUMN
   usage_json` migration idempotent + safe on a live pre-B2 DB, and do old NULL rows read as None without panic?
   Does a pre-B2 in-flight task (snapshot graph with no `panel` key, checkpoints with no usage_json) resume cleanly?

3. **Serde / wire back-compat.** `OrchEventKind::NodeFinished.usage`, `FrameKind::NodeFinished.usage`, and
   `WorkflowGraph.panel` all use serde default/skip-if-none. Are non-panel workflows + old journal rows + old spec
   snapshots byte-identical / still-deserializable? Any wire contract (`v` bump) silently broken? Does
   `encode_workflow_spec` round-trip `panel` through `WorkflowSpecEnvelope`?

4. **Template-var seam.** `{{workflow.costs}}`/`{{workflow.weights}}` are injected ONLY for fan-in nodes
   (`!n.inputs.is_empty()`). Confirm no collision with a real node id (NodeId bans `.`), no double-injection, and
   that an upstream node output literally containing `{{workflow.costs}}` is NOT re-expanded (single-pass renderer).
   Are the reserved vars correctly absent for root nodes?

5. **The render contract (SF-FIX-1).** `render_costs_table`: windowFraction = used/size as a raw fraction; per-field
   `n/a`; money cost only when present. Any panic on size==0, overflow, or NaN? Is the table header/shape stable so
   the synth can reproduce it verbatim?

6. **Concurrency / cancel / degrade.** The executor's fan-out drains in-flight nodes on cancel — does the usage
   3-tuple thread correctly through every early-return + the cancel branch? Does a failed/cancelled member produce
   a clean `n/a` row (degrade) without breaking the synth? Any torn read or lost usage under the FuturesUnordered
   scheduling?

7. **Ripple completeness.** All 5 TaskStore impls + the WorkflowSink impls + every `WorkflowGraph` literal + every
   `run_from*`/`node_finished` call site updated? Any `#[allow(clippy::too_many_arguments)]` that hides a smell?
   Any test that would pass even if the feature were broken (tautology)?

8. **Scope / deferrals.** Are the tracked deferrals (snapshot-replay `task watch` usage; live A2A SSE usage; JSON
   panel; native fan_out) cleanly NOT half-built? Anything that leaks a deferral into a broken half-state?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. A
BLOCKER is something that must change before merge. End with `BRANCH VERDICT: ship | fix-then-ship | needs-work`.
