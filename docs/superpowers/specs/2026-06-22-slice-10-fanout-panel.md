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

---

## v2 — dual spec-review folded (codex needs-revision + Opus needs-spike) — BINDING

> Supersedes the draft where it conflicts. Folds both lenses (strongly aligned) + an empirical probe. The probe
> RESOLVED the spike: a real codex turn in THIS env emits `used=15071, size=258400, windowFraction=0.058,
> cost=null` — so per-source USAGE/context-footprint is REAL for codex, but MONEY is claude-only. That reframes
> the marquee dimension (below). `Update::Usage` reaches the executor unconditionally (`acp_backend.rs:2473`,
> ignored at `executor.rs:275`), so per-node capture is feasible; a T1 one-node smoke test is the final proof
> (no further spike). SPEC NOW: **ready-to-plan.**

### SF-FIX-1 (Opus #1, codex #6, probe-confirmed) — the dimension is USAGE/context-size, NOT "cost"
Rename "cost" → **usage/context-footprint** everywhere. `UsageSnapshot.used/size` is CONTEXT-WINDOW OCCUPANCY
(`window_fraction = used/size`), not spend. The panel's per-source usage block = `{used, size, windowFraction}`
(REAL for both ACP agents) + a money `cost{amount,currency}` ONLY when the agent sent one (claude yes, codex
`null`). Per-field rendering: `used`/`size`/`windowFraction` when present else `n/a`; money only when present;
whole-`None` usage → all cells `n/a`. Do NOT sell "real cost" or assert money is non-zero in the gate. The five
panel dimensions become **pros / cons / usage(real) / benefit / risk** (the agents' qualitative analysis on
pros/benefit/risk; usage is the one REAL-DATA column).

### SF-FIX-2 (Opus #4 — the keystone) — WEIGHTS are operator config, not LLM-invented
The "weighted recommendation" weights come from the workflow TOML, NOT the synth's imagination (which would make
B2 "a prompt asking for a table" — the existing review/design synths already do that). Add a `[panel]` section to
the panel workflow config: `weights = { usage = 0.2, benefit = 0.4, risk = 0.3, ... }` (operator-assigned,
reproducible). Parse it, inject as a reserved `{{workflow.weights}}` synth var. The synth states the weights it
applied (markdown prose — stays ADR-0012-side: prose weights, NOT machine-emitted routing scores). THIS is what
makes B2 a real panel + more than a prompt tweak.

### SF-FIX-3 (codex #3) — reserved synth vars use an INVALID-NodeId namespace (no collision)
`{{costs}}`/`{{weights}}` would collide with an upstream node literally named `costs`/`weights` (node IDs become
template vars directly, `executor.rs:484`; `costs` is a valid `NodeId`). Use the `workflow.` namespace —
`{{workflow.costs}}`, `{{workflow.weights}}` — which is NOT a valid `NodeId` (verify the `NodeId` charset at
`ids.rs:58`; the `.` makes it un-collidable). Reserve the `workflow.` prefix for built-in synth context vars.

### SF-FIX-4 (codex #1+#2, Opus #3 — the BLOCKER) — per-node usage must survive crash-resume
Adding usage is additive at the wire/journal layer but is DROPPED through the fold→seed→`run_from` chain. The
FULL carrier (decide in plan, but the spec mandates one of these — NOT silent loss):
- Executor: a `NodeResult { output, ok, usage: Option<UsageSnapshot> }` (replaces the `(String,bool)` node output
  + the `HashMap<String,(String,bool)>` outputs map + the `run_from` seed type `(String,bool)` →
  `(String,bool,Option<UsageSnapshot>)`). `run_node` accumulates the FINAL pre-`Done` `Update::Usage` (the node's
  end-state; `executor.rs:169/275` stop ignoring it).
- `WorkflowEvent::NodeFinished{node,ok,output}` → `+ usage: Option<UsageSnapshot>` → the `WorkflowSink::
  node_finished` TRAIT signature gains a 4th param (BREAKING — update `DetachedProgressSink`, the streaming sink,
  test fakes).
- Durable: `OrchEventKind::NodeFinished{node,ok,output}` `+ usage` (versioned journal → old rows fold as `None`);
  `task_node_checkpoints` gains a nullable `usage_json` column (the established `ALTER TABLE ADD COLUMN` +
  PRAGMA-guard migration, `sqlite.rs:148-179`; old rows read `None`); `TaskProgressSnapshot.checkpoints` 4-tuple →
  carries `Option<UsageSnapshot>`; `fold_journal_to_snapshot` + the `resume_working_tasks` seed
  (`detached.rs:1423`) carry it.
- **RECOMMENDED minimum (cheaper, honest):** if the full seed-chain thread is too heavy, accept **wire+journal+
  checkpoint-COLUMN only** and have the RESUME SEED carry usage from the checkpoint (so a resumed-already-done
  node's usage is restored from its `usage_json` checkpoint, NOT re-run). A pre-B2 task / crashed-before-usage
  node seeds `None` → `{{workflow.costs}}` shows `n/a` for it. Plan picks the exact boundary set; either way old
  data folds as `None` with a test.

### SF-FIX-5 (codex #4, Opus #6) — SF-4 surfacing scope = detached task watch, gated on RAW fields
Per-source usage surfaces on the **detached `task watch`** structured `FrameKind::NodeFinished` (additive
`usage` field, `#[serde(skip_serializing_if="Option::is_none")]` → non-panel runs emit the identical frame, no
`v` bump). The LIVE A2A workflow SSE path emits only plain `node X ok` status text (`server.rs:1841`) — extending
THAT is a bigger change, **DEFERRED** (noted). The live-gate asserts the RAW `NodeFinished.usage` /
checkpoint `usage_json` (`used>0` when supported), NOT the synth markdown (which could echo/hallucinate). The
synth markdown must REPRODUCE the generated `{{workflow.costs}}` table verbatim (assert string-equality of the
injected table vs what appears in the synth output).

### SF-FIX-6 (codex #7) — D4 reword: the substrate is the WORKFLOW DAG, not fanout.rs
B2 reuses the **workflow DAG fan-out/fan-in + executor scheduling** (`executor.rs`), NOT the
`bridge-a2a-inbound/fanout.rs` coordinator (which serves the A2A direct-fan-out surface, a different path).
`fanout.rs` is adjacent precedent (source-identity/cancel already solved there), NOT B2's implementation
substrate. No native `fan_out` op in B2.

### Updated decisions
- D1 → SF-FIX-1 (per-field usage rendering; money only when present).
- D3 unchanged (markdown-first; JSON panel deferred — the deferred structuring node will consume the synth
  markdown + the SF-FIX-4 captured `usage` → `Part::data`; that's the tracked insertion point).
- D4 → SF-FIX-6 (workflow DAG, not fanout.rs).
- D6 (NEW): weights are operator config (SF-FIX-2), not LLM-emitted scores — keeps B2 ADR-0012-consistent.

### Updated live-gate
Run the new `panel` workflow with TWO intentionally-different members (codex@low + codex@high, or codex + claude)
+ a `[panel] weights` table → assert: (a) each non-synth node's RAW `usage.used > 0` (distinct per member,
proving real per-node capture — NOT from markdown); (b) the synth artifact reproduces the generated
`{{workflow.costs}}` usage table verbatim + states the configured weights + a weighted recommendation; (c) a
crash-resume (kill serve mid-run after the members finish, restart) still shows the members' usage in the resumed
synth's `{{workflow.costs}}` (proving SF-FIX-4); (d) degrade: one member fails → survivor synthesizes + its usage
shows `n/a`. T1 ships a one-node usage-capture smoke test as the foundation proof.

### Spike status: RESOLVED (probe + corpus + code). Ready-to-plan.
