# ADR-0038 — Agent memory as pinned context

**Date:** 2026-07-17
**Status:** Proposed (stub)

**Builds on:** ADR-0034 (`AgentDef`), ADR-0009 (stateless per-node turns), ADR-0032 (sandbox tiers).
**RFC:** [`../rfc-agents-workflows-part2-memory-delegation.md`](../rfc-agents-workflows-part2-memory-delegation.md) §1.

---

## Context

Every workflow node turn is stateless by construction, and persistence is deliberately shaped around it:
`turn_log` is content-free (its `(prompt_id, model, effort)` eval index assumes those determine the effective
prompt), and content-bearing rows are task-scoped and cascade-deleted. "Memory" — state that outlives tasks —
is therefore a **new retention class** (long-lived, content-bearing) and a **new input class** (prompt content
not derivable from the pinned definition). It must be designed, not bolted on.

## Decision

- Add `AgentDef.memory: Option<MemoryRef>` (`File` | `Store { scope }`), with
  `MemoryScope { Agent | AgentRepo | TaskLineage }`.
- New `MemoryStore` port (`bridge-core`), SQLite impl (`agent_memory` table in `bridge-store`), size-capped.
- **Read once at run start; pin into the run snapshot** alongside the graph (additive `"memory"` envelope
  field, serde-default; `SUPPORTED_SNAPSHOT_VERSION` stays 1). So resume replays the same memory the run
  started with, warm sessions ride the existing `NodeTurn.seed` seam, and batch runs read independently — all
  falling out of pin-by-value.
- **Close the eval hole by stamping:** add `memory_epoch`/`memory_sha` to `TurnContext` and `turn_log`
  (additive PRAGMA migration). Comparability becomes `(prompt_id, model, effort, memory_sha)` — memory is a
  *controlled variable*, not a silent corrupter.
- **M2 (declared distillation writes)** — memory written only as the terminal output of a *declared workflow*,
  provenance-stamped (`workflow:<id>@<def_hash>:<task_id>`), append-only/epoch-versioned. Seam defined here,
  built only against a concrete distillation use case.

## Consequences

- First long-lived content-bearing table; `turn_log` **stays content-free**.
- **M3 (in-turn read-write learning) is out of scope** (ADR-0039): it makes within-run streams
  non-reproducible and, decisively, opens **cross-tier memory laundering** (a tier-2 reader writing memory a
  tier-3 implementor trusts = a prompt-injection channel across the quarantine the tiers exist to enforce) —
  disqualified until a per-tier memory taint model exists.
