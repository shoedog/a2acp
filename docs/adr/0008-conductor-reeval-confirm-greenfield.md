# ADR-0008 — Conductor Re-evaluation: Confirm Greenfield (escalate via partial-adopt, not fork)

**Date:** 2026-06-01
**Status:** Accepted

**Closes:** the fork-vs-greenfield decision deferred by ADR-0002 (§Deferred) and ADR-0005 (§9). **Builds on:** ADR-0006 (bridge ACP-only) and ADR-0007 (the non-process API backend that supplied the missing evidence).

---

## Context

ADR-0002 built the bridge **greenfield** on the `agent-client-protocol` crate rather than forking `agent-client-protocol-conductor` (a Rust binary that orchestrates ACP **proxy-chains** — many proxies in front of many agents; its value is *composition*). It deferred the fork/adopt decision to "Increment 3, when proxy-chain composition becomes concrete," because forking would import composition abstractions before their pressure appeared, violating "build the layer you need, not the layer you might need."

ADR-0005 §9 set the **explicit re-evaluation criteria** and required a second protocol family + composition evidence before deciding. Both now exist: Increments 3a–3d shipped kiro, codex, gemini, and Claude (all ACP), plus a runtime-mutable registry, delegate, and fan-out; ADR-0007 added the **non-process** OpenAI-compatible API backend (a structurally different second backend kind).

## Decision

**Confirm greenfield.** Do **not** fork or adopt the conductor codebase. The hexagonal seams (`AgentRegistry`, `ConfigSource`/`ConfigStore`, `AgentBackend`, `PolicyEngine`, `RouteDecision`) stayed conductor-compatible, so the option is **not foreclosed**.

**Escalation path (chosen):** when genuine composition pressure appears (see Re-trigger below), **lead with partial-adopt — port specific conductor patterns into the greenfield seams — NOT a wholesale fork.** A full fork is rejected as the escalation: it would import ACP-proxy-chain machinery the evidence shows is unused, and the conductor has no model for the non-process backends the bridge is now growing.

## Evidence (criteria from ADR-0005 §9, weighed against what shipped 3a–3d + ADR-0007)

| Conductor-favoring signal | Appeared? | Evidence |
|---|---|---|
| Proxy-chaining | **No** | no proxy-chain code; `RouteTarget = { Local(id), Delegate, Fanout }` |
| Multi-hop agent graphs | **No** | Delegate is single-hop A2A; Fan-out is `(default, peer)` — no chains |
| Dynamic discovery | **No** | agents are static registry entries; no discovery layer |
| Shared cross-agent session/context | **No** | sessions are per-backend; no cross-agent context |
| Routing-policy bloat in `bridge-policy` | **No** | `bridge-policy` is **90 lines** (auth 37 + permission 49 + lib 4); `PolicyEngine` stayed a 1-method `decide()` |

| Greenfield-confirming signal | Held? | Evidence |
|---|---|---|
| Composition stays "select-by-id + fan-out/delegate" | **Yes** | exactly the three `RouteTarget` variants |
| Ports absorb each adapter without domain change | **Mostly** | gemini + Claude added with **zero** domain change (config / a code-*removing* retirement). The API backend needed a **bounded, mechanical** ripple (`cmd→Option`, `AgentKind::Api`, kind-aware `validate` — ~10 sites, one atomic commit) — the *intended* non-process signal, not port failure |

**The decisive insight — domain divergence.** The one adapter that required a domain change (the non-process API backend) is exactly where the conductor would **not** have helped: the conductor orchestrates *ACP process proxy-chains* and has no notion of a non-process HTTP backend (its model is spawned stdio proxies). The API-backend ripple is therefore evidence that **the bridge's problem domain — A2A ↔ *many kinds* of backend, including non-process — has diverged from the conductor's (ACP proxy-chain composition)**, not evidence for adopting it. Adopting now would buy composition machinery for pressure that never materialized.

## Re-trigger conditions (the use-cases / pressure that would lead to need)

Revisit this decision — and, per the chosen escalation, **lead with partial-adopt** — when any of these becomes concrete:

1. **Proxy-chaining / multi-hop agent graphs** — a request must traverse N agents/proxies in a chain or DAG, not "pick one + optionally fan-out/delegate."
2. **Dynamic agent discovery** — agents register/deregister at runtime from a discovery source (vs. static registry entries / hot-reloaded config).
3. **Shared cross-agent session/context** — multiple agents must share one session/context/memory within a task (vs. per-backend sessions).
4. **Routing-policy complexity** — `bridge-policy` / `RouteDecision` starts to bloat with conditional routing, per-hop policy, or capability negotiation.
5. **Self-hosting the dev workflow on the bridge (the LEADING candidate — Wesley's stated next priority).** Using the Rust a2a-bridge itself to drive **code reviews, plan/design reviews, spec reviews, research, and development** — i.e. reaching feature parity with, and replacing, the `~/code/a2a-local-bridge` Python PoC currently used black-box for the dual-review loop. This is multi-agent *orchestration* (submit task → dispatch to codex/claude/etc. → collect/compare results → review workflow) and is the most likely first real composition pressure. When this increment is taken up, **review the conductor's use-cases/patterns first** (a dedicated follow-on) and prioritize which to port into the greenfield seams.

Note: signal #5 is A2A-level **orchestration**, not ACP **proxy-chaining** — so even it may favor porting orchestration *patterns* over the conductor's proxy machinery. The conductor-pattern review (below) decides what's worth porting.

## Consequences

- **The parked decision is closed**: greenfield is the standing architecture; the registry/ports remain the spine.
- **The conductor option is preserved, downgraded to "partial-adopt on pressure."** A full fork is off the table unless the evidence inverts dramatically.
- **Two follow-ons are created** (neither blocks anything):
  1. **Conductor-pattern review** — examine `agent-client-protocol-conductor`'s use-cases and patterns and map each to (a) does the bridge want it, (b) when (which re-trigger), (c) port-as-pattern vs. ignore. This produces the prioritized "what to unlock" list Wesley asked for. (The conductor is an external public project — NOT under the `a2a-local-bridge` firewall — so it may be read directly.)
  2. **Self-hosting the dev workflow on the bridge** — its own brainstorm → spec → plan → build cycle; it is the leading re-trigger and the first place partial-adopt would be considered.
- **No code changes** from this ADR; it is a decision record.
