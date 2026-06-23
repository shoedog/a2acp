# Slice 10 — B2: Weighted Fan-out Panel — HANDOFF / Resume Doc

> The FIRST Slice-10+ tail item (all CORE slices 0–9 SHIPPED, `main` `4d8c66d`). **STATUS: architect DONE —
> spec v2 ready-to-plan (dual spec-review folded + spike RESOLVED).** Branch `feat/slice-10-fanout-panel`
> (base = `main` `4d8c66d`). Docs-only so far — NO production code written yet. Read top-to-bottom.

## ⏯️ RESUME POINT: write the PLAN next
- **Spec = `docs/superpowers/specs/2026-06-22-slice-10-fanout-panel.md`** — read the **`## v2`** section
  (BINDING; SF-FIX-1..6 + updated D1/D3/D4/D6 + the updated live-gate). It supersedes the draft above it.
- **NEXT:** write the implementation plan (`docs/superpowers/plans/2026-06-22-slice-10-fanout-panel.md`) per the
  `superpowers:writing-plans` skill → bite-sized TDD tasks realizing SF-FIX-1..6. Then dual plan-review (codex
  xhigh + Opus lens) → fold to plan v2 → TDD-implement task-by-task → whole-branch dual-lens review → live-gate
  → merge. (Same loop that shipped Slice 9.)
- Commit history so far: `6a038c7` (spec draft + spec-review scaffolding) → spec-review (codex port 8128 + Opus)
  → `f0dca7a` (spec v2 fold).

## What B2 is (the architect decision)
Turn the existing fan-out→synth workflow pattern into a **weighted panel**: N agents independently analyze the
same input; the synth compares them across **pros / cons / usage / benefit / risk** with **operator-configured
weights** and emits a **weighted recommendation** — markdown, surfaced with REAL per-source usage.

**Two forks, resolved (binding):**
1. **Markdown-first, NOT global JSON** (ADR-0012 honored). A machine-readable JSON panel is a DEFERRED
   structuring node (consumes the synth markdown + the captured per-node `usage` → `Part::data`) — built only
   when a deterministic consumer (CI gate / dashboard / dispatch) appears.
2. **A workflow PATTERN, NOT a native `fan_out` op.** Reuse the **workflow DAG fan-out/fan-in + executor
   scheduling** (`executor.rs`) — NOT `bridge-a2a-inbound/fanout.rs` (that's the A2A direct-fan-out path, a
   different surface; it's adjacent precedent only). A handle-aware native `fan_out` op with mid-stream
   continue / per-source-cancel-as-first-class is the bigger S3 lift — DEFERRED.

## The real CODE delta (concentrated, per spec v2)
1. **SF-FIX-4 (the BLOCKER / foundation):** capture per-node usage (the executor IGNORES `Update::Usage` today at
   `executor.rs:169/275`) and carry it THROUGH crash-resume. `NodeResult{output,ok,usage:Option<UsageSnapshot>}`
   + durable `OrchEventKind::NodeFinished{...,usage}` + a nullable `usage_json` checkpoint column (ALTER-ADD-
   COLUMN migration, old rows→`None`) + the resume seed/`run_from` carry it. The `WorkflowSink::node_finished`
   TRAIT gains a 4th param (BREAKING — update all impls). Plan picks the exact boundary set; recommended minimum
   = wire+journal+checkpoint-column, seed restores usage from the checkpoint (resumed-done nodes NOT re-run).
2. **SF-FIX-2 (the keystone):** `[panel] weights = {usage=.., benefit=.., risk=.., ..}` in the workflow TOML →
   parsed → injected as `{{workflow.weights}}`. Reproducible weights (NOT LLM-invented).
3. **SF-FIX-1:** the panel's usage block = `{used, size, windowFraction}` (REAL for ACP) + money `cost` ONLY when
   present (codex=`null`, claude=money). Per-field render; `n/a` when absent. Do NOT sell "real cost" / assert
   money non-zero.
4. **SF-FIX-3:** reserved synth vars use the `workflow.` namespace (`{{workflow.costs}}`, `{{workflow.weights}}`)
   — invalid `NodeId` → no collision with a node literally named `costs`/`weights` (`executor.rs:484` makes node
   IDs into template vars; verify charset at `ids.rs:58`).
5. **SF-FIX-5:** surface per-source usage on the **detached `task watch`** `FrameKind::NodeFinished` (additive
   `usage`, `skip_serializing_if=None` → non-panel runs byte-identical). Live A2A SSE plain-text path DEFERRED.
6. **SF-3:** a new `panel` workflow (config + `prompts/panel-synth.md`) instantiating fan-out→weighted-synth.

The panel format itself is prompt/config; the substantial code is #1 (per-node usage durable through resume) + #5
(wire surfacing). It IS a real foundation primitive (per-node usage) reusable by budgets/dashboards/the deferred
JSON node — not "just a prompt".

## Spike: RESOLVED
A real codex turn in THIS env (probe) emits `used=15071, size=258400, windowFraction=0.058, cost=null` →
per-source USAGE is real, MONEY is claude-only. `Update::Usage` reaches the executor unconditionally
(`acp_backend.rs:2473`, ignored at `executor.rs:275`) + the codex corpus has `usage_update` → capture is
feasible. **T1 ships a one-node usage-capture smoke test as the final proof — no further spike.**

## Live-gate shape (per spec v2)
`panel` workflow with 2 intentionally-different members (codex@low + codex@high, or codex+claude) + a `[panel]
weights` table → assert: (a) each non-synth node's RAW `usage.used>0` (distinct per member — from the
frame/checkpoint, NOT markdown); (b) the synth artifact reproduces the generated `{{workflow.costs}}` table
verbatim + states the configured weights + a weighted recommendation; (c) crash-resume mid-run still shows the
members' usage in the resumed synth's `{{workflow.costs}}` (proves SF-FIX-4); (d) degrade: one member fails →
survivor synthesizes + its usage = `n/a`.

## Proven loop + scaffolding + role matrix (reuse)
- **Roles:** codex gpt-5.5 HIGH implements (write, danger-full-access, **NO commit / NO git-mutating cmds**);
  codex gpt-5.5 XHIGH reviews (read-only sandbox); **Opus (controller)** architects/controls/**verifies in the
  clean host env (codex hits the `_dyld_start`/rustc-startup stall → controller re-runs runtime tests)**/commits/
  live-gates. codex = default implementor.
- **Scaffolding committed:** spec-review (`examples/a2a-bridge.slice-10-spec-review-codex.toml` port 8128 +
  `prompts/slice-10-spec-review.md`). For implementation reuse a `slice-9-impl`-style codex-HIGH config
  (`examples/a2a-bridge.slice-9-impl-codex.toml` port 8126 + `prompts/slice-9-impl.md` — re-point the prompt at
  the slice-10 plan, or copy to slice-10 variants). Whole-branch review → next free port.
- **STAGING DISCIPLINE:** stage ONLY each task's files. The worktree has MANY pre-existing untracked
  `examples/*.toml` / `prompts/*.md` + a pre-existing `M examples/a2a-bridge.slicing-analysis.toml` — NEVER fold
  them.
- **GOTCHAS to carry in:** (1) the unary `artifact.text` last-delta truncation — the live-gate must assert RAW
  frame/checkpoint usage, NOT the synth markdown text (codex spec-review #5). (2) `cargo test --workspace`
  catches stale cross-crate test counts that per-task `--no-run` + `--bin` filters miss (the Slice-9 MCP 6→8
  lesson) — run it before the whole-branch review. (3) the controller MUST re-run RUNTIME tests in the host env
  (codex's sandbox can't — the stall blocks them; Slice-9 shipped a wrong test assertion that way).

## Where the tail goes after B2
Per `docs/superpowers/specs/2026-06-17-orchestration-slicing.md:98` + the orchestration handoff: the remaining
Slice-10+ tail = E1 worktree-per-session · E6 retry/resume · E3 batch · E7 typed task-spec · E8 prompt-lib (all
independent; pick per value). Plus this slice's tracked deferrals: the JSON structuring node + the native
`fan_out` op + the live-SSE per-node usage surface.
