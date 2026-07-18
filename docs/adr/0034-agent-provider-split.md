# ADR-0034 — Agent / Provider split

**Date:** 2026-07-17
**Status:** Proposed (stub)

**Amends:** ADR-0005 §8 (config schema, additively). **Builds on:** ADR-0028 (per-agent MCP).
**Prerequisite:** [issue #35](https://github.com/shoedog/a2acp/issues/35) (registry reuse-criterion fix).
**RFC:** [`../rfc-agents-workflows.md`](../rfc-agents-workflows.md) §3.

---

## Context

`AgentEntry` (`domain.rs:121`) is one flat struct fusing three concerns — execution substrate
(adapter/sandbox/watchdog), model tuning, and agent-ish bits (tools, identity) — and carries **no role**
(role lives only in a workflow node's `prompt_template`). The fusion already forces duplication in shipped
configs (four+ entries over two binaries in `examples/a2a-bridge.tiers.toml`), where the role/tier/tool axis
multiplies against the substrate axis.

## Decision

- Split into `ProviderEntry` (substrate: adapter, sandbox, watchdog, model defaults) and `AgentDef`
  (`role_prompt`, `mcp`, model-default overrides, provider reference, identity), composed by
  `compose_agent(def, provider) -> AgentEntry` — producing exactly today's type.
- **Compose above the registry.** `bridge-registry`, `bridge-acp`, `bridge-container`, and the executor are
  untouched, keyed by `AgentId`, consuming the composed `AgentEntry`. This is required, not a shortcut: tools
  and `session_cwd` are spawn-frozen for codex (argv) and kiro (agent-config), so the real spawn unit is
  (provider × tools × cwd) ≈ today's entry; provider slot-sharing is only *sometimes* legal and is deferred.
- Role is prompt composition: `role_prompt ⊕ node.prompt_template` at the executor render site and the warm
  `NodeTurn.seed` seam. Add optional node-level `AgentOverride` (model/effort/mode), resolved by the existing
  `effective_config` layering (now three layers: node ⊕ agent-def ⊕ provider).

## Consequences

- Legacy `[[agents]]` entries synthesize an implicit `ProviderEntry` + `AgentDef` (golden parity test over
  every `examples/*.toml`). New `[[providers]]` syntax is additive; setting both `provider` and a substrate
  field on one agent is a load error.
- **Warm-session invalidation classification** (a tested contract, not folklore): a provider/tools change
  falls out of config-only reuse and forces respawn + lease-drain; a pure `role_prompt`/model-default edit is
  config-only and keeps the warm backend. This depends on the #35 fix, which adds `mcp`/`mcp_delivery`/
  `watchdog` to the reuse criterion.
- Provider slot-sharing by spawn-frozen `InstanceKey` and per-agent AgentCards are deferred, evidence-gated.
