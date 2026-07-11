# a2a-bridge documentation

> **Agents and operators:** start with the
> [`a2a-bridge-operator` skill](../skills/a2a-bridge-operator/SKILL.md). It contains the required
> preflight, provenance, failure-isolation, upgrade, and release workflow. The current tested agent
> matrix is in [`compatibility.md`](compatibility.md).

## Current status

- [`roadmap.md`](roadmap.md) — priority order and the pause/resume boundary.
- [`bridge-reliability.md`](bridge-reliability.md) — active reliability program and exit gates.
- [`compatibility.md`](compatibility.md) — dated host/container/model evidence and incident dispositions.
- [`m4-observability-roadmap.md`](m4-observability-roadmap.md) — M4 Slice 3b, reserved 3c, and the
  Slice 1/2 deferred-item ledger.

## Use the bridge

| Need | Source of truth |
|---|---|
| Agent-oriented quickstart and workflow commands | [`../AGENTS.md`](../AGENTS.md) |
| Install, configure, authenticate, and run | [`onboarding.md`](onboarding.md) |
| Choose host vs container and understand fallback limits | [`adr/0032-sandbox-tier-model.md`](adr/0032-sandbox-tier-model.md) |
| Container images, credentials, egress, Docker/Podman | [`containerized-agents.md`](containerized-agents.md) |
| Container MCP/LSP environment failure | [`containerized-mcp-env-trap.md`](containerized-mcp-env-trap.md) |
| CLI/config overview and troubleshooting | [`../README.md`](../README.md) |

## Understand or change the bridge

| Need | Source of truth |
|---|---|
| Protocol contracts | [`protocols.md`](protocols.md) |
| Architectural decisions | [`adr/`](adr/) |
| Current strategic analysis | [`2026-07-03-strategic-analysis.md`](2026-07-03-strategic-analysis.md) |
| Durable design/plan provenance | [`superpowers/README.md`](superpowers/README.md) |
| Frozen one-off development artifacts | [`history/README.md`](history/README.md) |

Design and handoff files are evidence for why the code exists. They do not override current code,
the current compatibility matrix, or the operator skill.
