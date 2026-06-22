# Slice 10 — B2: Weighted Fan-out Panel — SPEC

> Status: DRAFT (architect pass). The FIRST Slice-10+ tail item. Base = `main` `4d8c66d`.
> Loop: architect (this) → dual spec-review → plan → dual plan-review → TDD-implement → whole-branch review →
> live-gate → merge.

## Goal
Turn the existing fan-out→synth pattern into a **weighted panel**: when N agents independently analyze the same
input, the synthesis compares them across **pros / cons / cost / benefit / risk** and produces a **weighted
recommendation** — with **real per-source cost** (token/usage from Slice 2), not hallucinated. The output is a
human/LLM-readable markdown panel surfaced on the wire with per-source cost.

## Architect decision (the two design forks, resolved)
1. **Markdown-first, NOT global JSON (ADR-0012 honored).** B2's "weighted panel" is structured INFORMATION but
   ships as **structured MARKDOWN** (readable by the human/LLM orchestrators who actually consume reviews/designs
   today). A machine-readable JSON panel (scores/routing) is **DEFERRED** to a per-workflow structuring node
   (ADR-0012 seam (b)+(c): a structuring node + constrained output via the API backend) and built only when a
   real DETERMINISTIC consumer appears (CI gate, dashboard, dispatch). Do NOT impose JSON on all workflows.
2. **Panel = a workflow PATTERN, NOT a new first-class `fan_out` operation.** Fan-out already works (`fanout.rs`
   has source identity + per-source degrade + cancel; the workflow DAG already fans out via `inputs=[]` and fans
   in at a synth node). B2 is **UX on top** (the spec: "B2 is UX, not foundation"). A native handle-aware
   `fan_out` MCP/A2A op with mid-stream continue/per-source-cancel-as-first-class is the bigger S3 lift and is
   **DEFERRED**. B2 reuses the existing fan-out→synth substrate.

So B2's real CODE delta is small and concentrated: **capture + thread per-source COST** (the one panel dimension
that needs real data), then express the weighted comparison as a reusable synth contract + workflow, surfaced on
the wire.

## What ALREADY exists (do NOT rebuild)
- `fanout.rs`: `Source{id, stream}` + `run_with_cancel` — per-source identity (`Event.with_source`,
  `fanout.rs:127`), per-source failure degrade, cancel via `watch` + `finished[]` flags. ✅
- Workflow DAG fan-out/fan-in: nodes with `inputs:[]` run in parallel; a synth node `inputs:[a,b]` receives each
  upstream output as a template var (`{{a}}`, `{{b}}`, `{{draft}}` single-input alias) — `executor.rs:481-497`. ✅
- The `code-review`/`design`/`spec-review`/`plan-review` workflows ARE fan-out→synth (codex+claude→synth). ✅
- Slice 2 usage telemetry: ACP `usage_update` → `Update::Usage` → `session/status` usage block. ✅

## The gap B2 closes
- **Per-node usage is DROPPED in workflows.** The executor explicitly ignores `Update::Usage` at
  `executor.rs:169` and `:275` ("Slice 0: ignore"). So `WorkflowEvent::NodeFinished{node, ok, output: String}`
  carries NO cost → a panel's "cost" dimension would be hallucinated. **This is the foundation fix.**
- The synth receives only upstream TEXT, never each source's cost → it cannot weigh cost/benefit on real data.
- The final result surfaces the synth's markdown but no per-source cost breakdown.

## Design

### SF-1 — Capture per-node usage in the executor
`run_node` must accumulate the node's `Update::Usage` snapshots (the LAST one wins, per Slice 2 semantics — usage
is cumulative) instead of ignoring them, and attach the node's final usage to its result. Extend
`WorkflowEvent::NodeFinished` with `usage: Option<UsageSnapshot>` (additive; W3b checkpoint/journal must tolerate
the new field — confirm the snapshot fold + the durable `NodeFinished{output}` serialization stay
back-compatible). The node's `NodeOutput`/internal result struct carries `(output, ok, usage)`.

### SF-2 — Thread per-source cost into the synth's input
At fan-in (`executor.rs:481-497`), in addition to each upstream node's text (`{{a}}`), inject a **cost context**
the synth can weigh — e.g. a `{{costs}}` template var rendering a small per-source cost table
(`source | tokens | cost`) built from each input node's captured `usage`. The synth prompt references it. Keep
the existing `{{a}}`/`{{draft}}` vars unchanged (back-compat for current workflows that don't use `{{costs}}`).

### SF-3 — The weighted-panel synth contract (a prompt + workflow)
A reusable **`panel` workflow** (new `examples/` config + a `prompts/panel-synth.md`) that fans out N agents (the
panel members) over the same input, then a synth node produces the **weighted panel**: for EACH source a compact
`{ pros / cons / cost (from {{costs}}) / benefit / risk }` block, then a **weighted recommendation** that states
the WEIGHTS used and the winner + why. Markdown. (Also offer a panel-format variant of the existing review/design
synth prompts if low-risk — but a standalone `panel` workflow is the deliverable.)

### SF-4 — Surface per-source cost on the wire
The detached/streaming result must expose each source's cost. Minimum: `NodeFinished` (W3b
`NodeFinished{output}` → `NodeFinished{output, usage}`) carries it, and `task watch` / the workflow progress
frame renders a per-node `usage`/cost field (additive frame field, serialize-only). The final panel artifact is
the synth markdown (which now includes the real costs via `{{costs}}`).

## Decisions (resolve in dual-review)
- **D1:** cost UNIT — surface raw token counts always; surface a money `cost{amount,currency}` ONLY when the
  agent's `usage_update` carried one (Slice 2 made cost optional; claude sends cost, codex may not). The
  `{{costs}}` table shows tokens always, money when present. Don't fabricate money from tokens.
- **D2:** `NodeFinished.usage` is `Option<UsageSnapshot>` (None when the backend emitted no usage — e.g. an api
  backend or a crashed node). The synth's `{{costs}}` table shows "n/a" for None.
- **D3:** Panel format is MARKDOWN (ADR-0012). A JSON `Part::data` panel is a tracked DEFERRAL (structuring node
  + constrained output, per-workflow, when a deterministic consumer exists).
- **D4:** Reuse the existing fan-out substrate — NO new `fan_out` native op, NO handle-aware mid-stream
  continue/per-source-cancel-as-first-class (deferred S3 lift).
- **D5:** W3b durability — the new `usage` field must be ADDITIVE to the `task_node_checkpoints` snapshot + the
  versioned journal so crash-resume of a pre-B2 task still folds (mirror the W3b additive-migration discipline).

## Out of scope (tracked deferrals)
- Machine-readable JSON panel / scores / routing (ADR-0012 structuring node — until a deterministic consumer).
- Native first-class `fan_out` MCP/A2A operation with handle semantics + mid-stream continue.
- Per-source cancel as a first-class operation (fan-out cancel already works at the workflow/`fanout.rs` level).
- Cost BUDGETS / caps on a panel (a future cost-governance slice).

## Live-gate shape (vs real agents)
Run the new `panel` workflow over a real question with codex + claude (or codex + a 2nd codex at different
effort) as panel members → the synth artifact shows, for each source, pros/cons/**real cost**/benefit/risk + a
weighted recommendation naming the weights + winner; `task watch` (or the result) shows each source's **real
token/cost** (distinct per source, matching what the agents actually consumed — assert the costs are non-zero +
differ, proving they're real usage not hallucinated). Degrade case: if one panel member fails, the panel still
synthesizes from the survivor + notes the missing lens + its cost shows "n/a".

## Open questions for the dual spec-review
- Q1: Is extending `WorkflowEvent::NodeFinished` (a W3b-durable type) with `usage` truly additive-safe for
  crash-resume of in-flight pre-B2 tasks? Trace the `task_node_checkpoints` snapshot + the journal fold.
- Q2: Is `{{costs}}` (a synth template var) the right seam for cost, or should per-source cost ride each source's
  output text? (Template var keeps source text clean + is opt-in for back-compat.)
- Q3: Does capturing per-node usage interact with the W3b `run_from(seed)` resume (a resumed node re-runs — does
  it re-capture usage, and is the re-run's cost the one that counts)?
- Q4: Scope check — is the markdown-first + reuse-substrate cut correct, or does "generalized fan-out" demand the
  native `fan_out` op in-slice?
