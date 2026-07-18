# ADR-0039 — The planner line (decision-not-to-build)

**Date:** 2026-07-17
**Status:** Proposed (stub) — decision-not-to-build (ADR-0012 style)

**Upholds:** ADR-0008 (no orchestrator inside the bridge; the orchestrator is the caller) and its 5-condition
re-trigger rule.
**RFC:** [`../rfc-agents-workflows-part2-memory-delegation.md`](../rfc-agents-workflows-part2-memory-delegation.md) §2.3–2.4, §3.
**Related issue:** [#36](https://github.com/shoedog/a2acp/issues/36) (the loopback door).

---

## Context

The Agent/Provider split and sub-workflow composition (ADR-0034/0036) move the bridge one deliberate step
toward an "agent runtime." This ADR fixes the line so "should the bridge plan?" is not re-litigated ad hoc.
The bridge stops being a bridge the moment either is decided by an agent **at runtime** instead of by a pinned
definition: **execution topology** or **prompt-content lineage**. Cross either and pin-by-value resume, eval
reproducibility, and the tier containment model all fail together.

## Decision

- **Non-goals, pending real evidence** (partial-adopt only under ADR-0008's re-trigger rule, never a wholesale
  pivot):
  - **D-c — runtime, agent-initiated, LLM-chosen delegation** (an agent decides mid-turn to invoke another
    agent/workflow). Re-triggers ADR-0008 #1/#2/#4 (and #3 with memory); no present evidence — every operator
    use case is declared-topology.
  - **M3 — read-write learning memory** (agents write memory mid-turn). Re-triggers #3/#4 and is independently
    disqualified by cross-tier laundering (ADR-0038).
- **Supported "agents calling agents" path:** an *external* caller driving `a2a-bridge mcp` — precisely
  ADR-0008's model (the planner is the caller; the bridge stays a deterministic executor).
- **Loopback is unsupported and guarded.** A bridge-managed agent whose `[[agents.mcp]]` points at the
  bridge's own MCP can invoke `run_workflow` mid-turn today, unguarded (issue #36). Enforcement: stamp a
  call-depth marker (env var / MCP `clientInfo`) through `a2a-bridge mcp` spawns and have the Coordinator
  reject `run_workflow` above depth 1 unless explicitly configured. Fail loud.

## Consequences

- If the operator later genuinely wants the agent-framework fork, that is a new project decision made against
  ADR-0008's escalation rule with the trade named explicitly: adaptivity purchased with reproducibility,
  resumability, and the containment model — not an increment.
