You are doing a rigorous, adversarial SPEC REVIEW (read-only) of "Slice 10 — B2: Weighted Fan-out Panel" for the
a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). READ-ONLY: read the spec + the real code;
do NOT edit/build/test.

The spec: `docs/superpowers/specs/2026-06-22-slice-10-fanout-panel.md`. Binding context:
- Fan-out engine: `crates/bridge-a2a-inbound/src/fanout.rs` (Source identity + cancel ALREADY work).
- Workflow executor (fan-out/fan-in/synth): `crates/bridge-workflow/src/executor.rs` — note per-node usage is
  IGNORED today at ~`:169`/`:275`; `WorkflowEvent::NodeFinished{node, ok, output}` ~`:80-87`; fan-in template-var
  injection ~`:481-497`.
- W3b crash-resume durability: per-node checkpoints + versioned journal (the `NodeFinished{output}` durable type).
- ADR-0012 (`docs/adr/0012-structured-output-deferred.md`): markdown-first; structure only at a deterministic
  boundary via a structuring node + constrained output.
- Slice 2 usage telemetry (`UsageSnapshot`, optional `cost{amount,currency}`).

{{input}}

GROUND every finding in real `file:line`. Pressure-test:
1. **The architect cut (D3/D4).** Is "markdown-first weighted panel + reuse the fan-out substrate (no native
   fan_out op, no JSON)" the RIGHT scope, or does it under/over-deliver "generalized fan-out + weighted panel"?
   Is the ADR-0012 resolution sound?
2. **SF-1 per-node usage capture.** Is extending the W3b-durable `NodeFinished` with `usage` truly ADDITIVE-safe
   for crash-resume of an in-flight PRE-B2 task? Trace the `task_node_checkpoints` snapshot serialization + the
   journal fold + the `run_from(seed)` resume. Will a resumed node re-capture usage correctly (Q3)?
3. **SF-2 cost threading.** Is `{{costs}}` (a synth template var) the right seam, or does it collide with the
   existing `{{a}}`/`{{draft}}` var injection? Back-compat for current workflows that don't reference `{{costs}}`?
4. **Usage semantics.** Is "last Usage snapshot wins" correct for a node (cumulative usage)? What about a node
   with NO usage (api backend / crash) → Option<UsageSnapshot>=None? D1 (tokens always, money only when present)?
5. **Live-gate reachability.** Can the gate actually prove the costs are REAL (non-zero, differ per source) and
   not hallucinated? Is the degrade case (one member fails → survivor + "n/a" cost) reachable?
6. **Missing pieces / wrong anchors / SF gaps.** Anything the spec must add (e.g. the streaming/detached
   surfacing of per-source cost), any wrong `file:line`, any SF that won't realize its goal?
7. **Spikes.** Does anything need an empirical spike before planning (e.g. does codex-acp emit `usage_update`
   per workflow node turn, so the cost is actually capturable)?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + `file:line` + a concrete fix. Answer
Q1-Q4 from the spec. End with `SPEC VERDICT: ready-to-plan | needs-revision | needs-spike`.
