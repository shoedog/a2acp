# ADR-0037 — Peer node targets

**Date:** 2026-07-17
**Status:** Proposed (stub)

**Builds on:** ADR-0036 (`NodeTarget`), the existing `DelegationPort`/`PeerDelegation` (`RouteTarget::Delegate`).
**RFC:** [`../rfc-agents-workflows-part2-memory-delegation.md`](../rfc-agents-workflows-part2-memory-delegation.md) §2.2.

---

## Context

Static, declared delegation to a remote A2A peer already exists as a top-level route
(`RouteTarget::Delegate` → `DelegationPort::delegate`/`cancel`, `bridge-a2a-outbound`). Making it a *node
target* lets a declared DAG include a remote brain as one hop — the greenfield-build branch of ADR-0008 #1
(a declared multi-brain graph), not runtime routing.

## Decision

- `NodeTarget::Peer(PeerId)` in the executor's node runner: render the prompt → `DelegationPort::delegate` →
  drain the SSE event stream into node output → checkpoint. The node's cancel token maps to
  `DelegationPort::cancel`. No executor signature change.
- v1 restricts `Peer` to the single configured `[delegation]` peer; multi-peer `[[peers]]` is additive later.

## Consequences

- **Resume re-delegates:** a resumed peer node re-submits to the remote (mirrors how any resumed node
  re-prompts, but the side effect is remote — note the idempotency expectation on the peer).
- **Caller-identity forwarding stays deferred** (the code says so, `bridge-a2a-outbound/src/lib.rs:21-22`): a
  peer node runs under the bridge's configured bearer, not the original caller's identity. Fine for a
  single-operator deployment; must be closed before any multi-tenant story.
- Distinct from runtime agent-initiated delegation, which remains a non-goal (ADR-0039).
