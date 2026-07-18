# ADR-0035 — Workflow definition store: deferred

**Date:** 2026-07-17
**Status:** Proposed (stub) — decision-not-to-build (ADR-0012 style)

**Builds on:** ADR-0005 (deferred `ConfigStore` write-back, `ports.rs:386`), ADR-0033.
**RFC:** [`../rfc-agents-workflows.md`](../rfc-agents-workflows.md) §2.4.

---

## Context

The DAG editor (TUI/mobile/MCP) needs a write path for workflow definitions. ADR-0005 already declares a
`ConfigStore: ConfigSource { upsert; remove }` seam "defined now, impl'd later." The temptation is a
DB-backed definition store separate from the config file.

## Decision

- **The config file remains the single source of truth.** Editor writes go through a `WorkflowConfigStore`
  (extending the deferred 3b.2 write-back seam): validate against the current registry snapshot + DAG rules,
  then persist via surgical `toml_edit` + atomic rename. The existing file watcher is the apply path
  (ADR-0033) — there is no parallel store to drift against.
- Exposed as `Coordinator` methods → the `a2a-bridge mcp` adapter gets `workflow upsert/remove/list`; a CLI
  verb follows. **No admin HTTP routes in `serve`** (consistent with the operator-UI admin-sidecar stance).
- A DB/remote definition store is **deferred**. Re-trigger: concurrent multi-writer editing or a multi-host
  deployment. Until then, two SSOTs is the real risk and is avoided.

## Consequences

- Every write is file → watcher → catalog, so there is exactly one source of truth and no split-brain. Resist
  any "fast path" that applies to the catalog directly and writes the file second.
