# Conductor Pattern Review — what to port, when, what to ignore

**Date:** 2026-06-01
**Follows:** ADR-0008 (confirm greenfield; escalate via partial-adopt). This is the prioritized "what to unlock" list that ADR-0008 §Consequences created as a follow-on.
**Sources:** the upstream ACP proxy-chains RFD (`agentclientprotocol.com/rfds/proxy-chains`); `agent-client-protocol-conductor` (crates.io / `symposium-acp`: `sacp` / `sacp-proxy` / `sacp-conductor`); the bridge's own analysis (`a2a-bridge-analysis.md` §8.1). The conductor is external/public — not under the `a2a-local-bridge` firewall, so read directly.

---

## The one structural insight that drives every verdict

The conductor and the bridge **compose on different axes**:

- **Conductor** inserts behavioral **shims (proxies) in front of ONE agent**: `Editor → Conductor → Proxy₁ → Proxy₂ → Agent`. The conductor is a hub that routes every message through itself (`proxy/successor`, proxy isolation), presenting the chain as a single ACP agent. Its value is *extending one agent's behavior without modifying it*.
- **Bridge** dispatches to **MANY agents and composes their results into a workflow**: A2A inbound → registry select-by-id / fan-out / delegate → N backends. Its value is *orchestrating many agents* (and now many *kinds* of backend, incl. non-process).

So most conductor patterns are **proxy-chain (vertical: shim one agent)**, while the bridge — and especially the leading re-trigger (self-hosting the dev/review workflow) — needs **orchestration (horizontal: many agents, one workflow)**. The two rarely overlap. Where a conductor idea *is* useful to the bridge, it ports as **in-process middleware**, never as the proxy-chain spine.

## The map (through the self-hosting lens)

| Conductor pattern | What it is | Bridge wants it? | Trigger | **Verdict** |
|---|---|---|---|---|
| **Proxy-chain composition** (the core: conductor-as-hub, `proxy/successor`, proxy isolation) | Insert shims between editor and one agent; present as one agent | The bridge's registry + fan-out + delegate already own its composition, on a different axis | re-trigger #1 (multi-hop) only | **IGNORE as architecture.** Do not adopt the proxy-chain spine; it answers a question the bridge isn't asking. |
| **Skill / plugin = on-demand instruction injection** | A proxy prepends instructions when a skill is invoked | Yes — the bridge already has "skills" (fan-out, delegate); reviews/research/dev are skills | **self-hosting (#5)** | **PORT-AS-PATTERN — MEDIUM, the most relevant.** Borrow the *shape* (a skill = request-shaper: prompt template + agent selection + output handling) as in-process middleware when building self-hosting. |
| **Subagent coordination** (a proxy spawns/initiates sibling sessions) | One component drives sibling agents | Yes — this IS the self-hosting core | **self-hosting (#5)** | **EXTEND-EXISTING (greenfield).** Generalize fan-out to N registry entries + collect/compare. Port the *idea*, implement on the bridge's own fan-out/registry, NOT the conductor's proxy model. |
| **Context-injection proxy** (prepend persistent session data/instructions) | Inject context before the agent sees it | Partially — the bridge already injects via prompt `parts`; a first-class pre-prompt injector (diff/spec/rubric) could help | self-hosting (#5) | **PORT-AS-PATTERN — LOW.** A small in-process "context injector" hook, only if self-hosting reviews need structured context beyond the prompt. |
| **Tool-filtering / coordination proxy** | Restrict which tools an agent may use | Maybe — read-only review wants tool restriction; bridge is no-fs today (blunt filter) | re-trigger #4 (policy complexity) | **PORT-AS-PATTERN — LOW.** A per-request capability/tool policy as a `PolicyEngine`/`RouteDecision` extension — watch for `bridge-policy` bloat as the signal. |
| **Response-transformation proxy** (process/filter outputs) | Transform/filter agent output | The translator already maps/coalesces; structured-finding extraction belongs in the self-hosting workflow | self-hosting (#5) | **EXTEND-EXISTING — LOW.** Do it in the workflow/translator, not a proxy. |
| **MCP bridging / IDE capability exposure** (stdio↔TCP MCP; diagnostics/fs/terminal) | Bridge MCP servers so agents get fs/tools | No, for now — the bridge is deliberately no-fs; self-hosting reviews are read-only | (fs-proxying decision, ADR-0006) | **IGNORE / revisit with the separate fs-proxying decision.** Not needed for self-hosting. |
| **`peer` routing + peer-enumeration discovery** (upstream *proposed* future) | Route `proxy/successor` to a named peer; enumerate peers for discovery | Only if the bridge needs multi-agent routing/discovery beyond registry-by-id | re-trigger #1/#2 | **WATCH, don't build.** Let it land upstream first; the bridge's registry-by-id covers selection today. |
| **Hierarchical nesting** (conductor-in-proxy-mode → trees) | Tree topologies of proxy chains | No use-case | re-trigger #1 | **IGNORE.** |
| **Shared cross-agent context** (proxies maintain conversation-level state across agents) | Agents share one context within a task | No — for self-hosting reviews, agent **independence** is the value (Codex and Claude review the same input independently; see review-agent-roles) | re-trigger #3 | **IGNORE for now.** Don't couple the reviewers. |
| **Proxy isolation / single-hub routing** | Internal conductor mechanism | N/A unless adopting the spine | — | **N/A.** |

## Headline conclusion (the "what to unlock" answer)

1. **Do NOT adopt the conductor's proxy-chain spine** — its core composition axis is orthogonal to the bridge's. This reinforces ADR-0008.
2. **The leading re-trigger (self-hosting) does NOT call for conductor adoption either** — it calls for **extending the bridge's own fan-out / registry / skill primitives** (subagent-coordination, response-handling), optionally borrowing **two in-process *shapes*** from the conductor:
   - **`skill = request-shaper`** (MEDIUM) — the cleanest borrow; shapes the review/research/dev request (prompt + agent + output handling).
   - **`context-injector` middleware** (LOW) — only if structured context-injection is needed.
3. **Three patterns are real but gated on *other* re-triggers**, not self-hosting: tool-filtering (→ policy-complexity #4), `peer`-routing/discovery (→ multi-hop/discovery #1/#2), shared-context (→ #3). **Watch, don't build.**
4. **Everything else is ignore / not-applicable** (proxy-chain spine, hierarchical nesting, MCP-bridging-for-now, proxy isolation).

**Net:** the self-hosting increment is a **greenfield extension of existing primitives** + at most one or two borrowed in-process *shapes*. Nothing here changes ADR-0008; it sharpens it — even the leading pressure doesn't justify adopting the conductor, only borrowing a couple of its patterns as middleware.
