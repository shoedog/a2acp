# ADR-0036 — Sub-workflow composition by flattening

**Date:** 2026-07-17
**Status:** Proposed (stub)

**Amends:** ADR-0009 (node model). **Preserves:** ADR-0015 (streaming/reattach), ADR-0024 (two engines).
**Builds on:** ADR-0033/0034.
**RFC:** [`../rfc-agents-workflows-part2-memory-delegation.md`](../rfc-agents-workflows-part2-memory-delegation.md) §2.1.

---

## Context

Workflows want to reuse sub-structures (a shared reviewer/synth pair) as composable units — "agents/workflows
calling workflows." The question is whether a node targeting another workflow stays inside the declared,
pinned, acyclic model or introduces runtime recursion.

## Decision

- Generalize the node target: `NodeTarget { Agent(AgentId) | Workflow(WorkflowId) | Peer(PeerId) }`
  (back-compat: the existing `agent =` field parses as `Agent`; exactly-one-of, loud otherwise).
- **Expand at submit, do not recurse at runtime.** `run_workflow` resolves `Workflow` targets transitively and
  inlines each child graph into the parent: child node ids namespaced (`sub.correctness`), the child's single
  terminal (guaranteed by `validate()`) renamed to the composite node's id so parent `inputs` resolve
  unchanged, child roots fed the composite node's rendered `prompt_template`. The result is **one flat
  `WorkflowGraph`**, pinned by value exactly as today.
- Two static validation layers: `catalog.apply` checks the **reference graph** (edges = `Workflow` targets)
  acyclic (same Kahn algorithm as `assert_acyclic`), then `validate()` runs on the flattened graph.
- v1: `retry`/`overrides` on a `Workflow`-target node are a load error (child nodes own their own).
- `def_hash` is computed over the **flattened** form (a child edit changes every parent's effective hash).

## Consequences

- Durability/resume, streaming/reattach, and the two-engines constraint are all **unchanged by construction** —
  flattening happens above the executor, which still takes one flat `Arc<WorkflowGraph>`; a mid-child crash
  resumes inside the child via the existing per-node checkpoints.
- This is authoring reuse (a macro over declared graphs). Nothing about execution becomes dynamic. Child-task
  recursion (a sub-run as its own `TaskRecord`) is explicitly rejected — it adds nested resume/journal/cancel
  machinery for zero declarative gain.
