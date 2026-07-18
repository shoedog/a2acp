# ADR-0033 — Runtime workflow definitions

**Date:** 2026-07-17
**Status:** Proposed (stub)

**Amends:** ADR-0009 (the "load-once at boot" clause only). **Preserves:** ADR-0015, ADR-0024.
**RFC:** [`../rfc-agents-workflows.md`](../rfc-agents-workflows.md) §2 (full design, code refs, phased plan).

---

## Context

Workflows are read once at boot and frozen into `Coordinator.workflows` (`coordinator.rs:168`), while only
`[[agents]]`/`[registry]` hot-reload. ADR-0009 justified this as removing a `RegistrySnapshot`-vs-workflows
TOCTOU. But the operator wants a DAG builder whose saved workflows apply live, and the analysis found the
supposed hard problem is already solved: a run **pins its entire graph by value** into the durable task record
(`workflow_spec_json`, `detached.rs:1420`), the executor takes that snapshot per run and never re-reads the
map, and crash-resume replays from the snapshot. Boot-load therefore protected only *validation-time*
referential integrity, not run-time integrity.

## Decision

- Introduce a `WorkflowCatalog` (an `ArcSwap` map of `VersionedGraph { graph, def_hash }`) replacing the frozen
  `HashMap`. Workflows own no runtime resources, so this is an atomic map swap, not a slot machine like the
  registry.
- Derive registry + workflows from **one** parse of the config file (`ConfigSnapshot`), and apply them in
  order (`registry.apply` then `catalog.apply`) on every reconcile — re-establishing referential integrity on
  every edit, strictly stronger than the one-time boot check. No second file watcher.
- Keep **run-pinning-by-value** as the immutability mechanism (canonizes existing `encode_workflow_spec`
  behavior). The executor signature is unchanged — it still takes `Arc<WorkflowGraph>` per run, so the engine
  can never observe a def mutation mid-run. Add an additive `def_hash` to the snapshot envelope for provenance
  (`SUPPORTED_SNAPSHOT_VERSION` stays 1).
- Rebuild the per-workflow Agent Card skills on `catalog.apply` (reuse the SIGHUP catalog-refresh path).

## Consequences

- `[[workflows]]`/`[[prompts]]` edits start applying live — a behavior change; ship behind
  `[workflows] hot_reload = true` for one release, then default-on. `[server]`/`[store]` stay boot-read.
- In-flight and resumable runs are unaffected by definition edits, by construction.
- **Guard against regression:** any future change that makes the executor or resume path read the catalog
  instead of the run snapshot re-opens the mid-run-mutation hazard. Test: delete a def mid-run, assert both
  completion and resume.
