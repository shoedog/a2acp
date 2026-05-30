# ADR-002 — Greenfield on `agent-client-protocol` Crate, Not a Conductor Fork

**Date:** 2026-05-30
**Status:** Accepted

---

## Context

The analysis document (`a2a-bridge-analysis.md`, §1, §8.1, §9.6) originally recommended forking `agent-client-protocol-conductor` as the starting point for the bridge. The conductor is a Rust binary that orchestrates ACP proxy chains (many proxies in front of one or more agents), and its architecture is architecturally close to the target destination for a multi-agent bridge.

The **Addendum dated 2026-05-29** in `a2a-bridge-analysis.md` revises that recommendation:

> "The headline 'fork or vendor agent-client-protocol-conductor' should be read as 'converge on the conductor architecture; adopt its codebase when composition pressure justifies it.' For Increments 1–2 (the walking skeleton), build greenfield on the agent-client-protocol crate rather than forking the conductor."

The core argument: the conductor's value is *composition* — routing messages through many proxies to many agents. A one-agent, zero-proxy walking skeleton exercises none of that machinery. Forking it at this stage imports abstractions whose purpose has not yet appeared, which violates the seam-discipline principle "build the layer you need, not the layer you might need" (`seam-discipline.md`, v3 §8).

The design spec (`docs/superpowers/specs/2026-05-29-a2a-bridge-v1-design.md`, §2, Decision table) locks this in: "Spine construction: Greenfield on agent-client-protocol crate (not a conductor fork)."

---

## Decision

The v1 walking skeleton is built **greenfield** using the `agent-client-protocol` crate as the ACP client library directly, with a hexagonal architecture whose port boundaries (§4.2 of the design spec) are designed to be compatible with eventual conductor adoption.

The fork-versus-greenfield evaluation is **deferred to Increment 3**, when the second and third CLI agents arrive and proxy-chain composition becomes concrete. At that point, the evaluation has empirical data (from Increments 1–2) about where the seams are actually load-bearing.

---

## Consequences

**Positive:**

- Skeleton work focuses entirely on validating protocol plumbing (A2A ↔ ACP translation, subprocess lifecycle, framing). This is what needs validation at Increment 1–2; conductor composition does not.
- No conductor-specific abstractions are imported before they have a use case. This keeps the codebase small and the seams clean.
- The greenfield approach makes it easier to evaluate the conductor's abstractions objectively at Increment 3, since there is no sunk cost in a fork.
- The hexagonal port traits (§4.2) are conductor-compatible by design: `AgentBackend`, `RouteDecision`, `PolicyEngine` all have signatures that could be backed by conductor-derived implementations without changing their callers.

**Negative:**

- Some conductor plumbing (proxy-chain composition, multi-agent routing) will need to be written from scratch if the Increment-3 evaluation concludes "continue greenfield." This is accepted as the cost of the principled evaluation.
- If the conductor API changes between now and Increment 3, the greenfield code may need adaptation. Mitigated by the port traits isolating the bridge's internal model from the conductor's model.

**Deferred:**

- Increment 3: conductor adoption decision. Reasonable outcomes: fork the conductor; continue greenfield; partially adopt conductor concepts without forking. See design spec §12 for the framing.
