# ADR-0014 — Session cwd / Per-Request Repo Targeting

**Date:** 2026-06-03
**Status:** Accepted

**Builds on:** ADR-0013 (containerized agents + egress) — the deployment posture this increment makes ergonomic. The one bridge-code change needed to run agents against many codebases from one `serve`.

---

## Context

The bridge points an agent at a working directory via a single config value that fed the ACP `session/new` cwd (ACP §11A requires an absolute session cwd). Two needs exceeded that:
1. **Containerized agents** operate at an in-container path (e.g. `/work`); coupling the session cwd to a host path forced an **identical-path mount hack** (`-v repo:repo`).
2. **Per-request repo targeting** — one `serve` driving agents across many codebases, the target chosen per request — was impossible (cwd was fixed per agent).

## Decision

**cwd is a session *location*, not LLM config — and it is set per request.**
- A validated **`SessionCwd`** newtype (parse-don't-validate: absolute, lexically normalized, NUL-free) makes validity a type guarantee — the mint, the inbound gate, and resume all receive a guaranteed-valid value.
- The per-session stash becomes **`SessionSpec { config: EffectiveConfig, cwd: Option<SessionCwd> }`**, threaded through the **existing** `configure_session`→`ensure_session` mint timing (reuse the seam, separate the type). cwd is **NOT** folded into `EffectiveConfig`.
- The per-request cwd is a **distinct `RoutedCall.session_cwd`** (NOT `AgentOverride`, whose `{model,effort,mode}` are dropped for workflows).
- Workflows thread a **`WorkflowRunContext { session_cwd }`** to every node — for **both** the streaming (`spawn_workflow_producer`→`run`) and detached (`spawn_detached_workflow`→`run_from`) paths; the executor stays pure (the context is a forwarded value, never read by scheduling).
- Resolution at mint: **request cwd → static `session_cwd` → `cwd` → `"."`**.
- The detached path persists `tasks.session_cwd` (its own additive column, W3b migration pattern — not in the `{"v":1,graph}` snapshot) and **re-validates** it on resume (corrupt → `Interrupted`).
- A **session's cwd is immutable after `session/new`** (ACP §11A): reusing a warm session with a different cwd → `InvalidStateTransition`, never a silent stale-cwd serve.
- Optional **`allowed_cwd_root`** gates a per-request cwd to a configured subtree (component-wise, lexical — a path-shape guard, not a sandbox).

## Components

- **`bridge-core`:** `SessionCwd` newtype; `SessionSpec`; `AgentEntry.session_cwd`; `TaskRecord.session_cwd`; `AgentBackend::configure_session(&SessionSpec)`.
- **`bridge-acp`:** stash `HashMap<SessionId, SessionSpec>`; `ensure_session` mints `spec.cwd ?? static`; records the minted cwd + the immutability guard.
- **`bridge-workflow`:** `WorkflowRunContext`; `run_with_context`/`run_from_with_context` (`run`/`run_from` delegate with default); `run_node` builds the per-node `SessionSpec.cwd`.
- **`bridge-a2a-inbound`:** parse + validate `a2a-bridge.cwd` → `RoutedCall.session_cwd` (rejected before mint); single-agent + both workflow dispatch paths apply it; persist at detached submit; re-validate + restore on boot resume.
- **`bridge-store`:** additive `tasks.session_cwd` column via `migrate_tasks_columns` + all three SELECTs + `row_to_task`.
- **`bin/a2a-bridge`:** `AgentEntryToml.session_cwd`, global `allowed_cwd_root`; a unit-tested `resolve_static_session_cwd` helper; documents that the host child has no cwd.

## Provenance — dual-design + dual-review + a settling probe

rev1 folded cwd INTO `EffectiveConfig` (reuse the machinery). Before building, a **firewalled independent codex design** (which never saw rev1) AND **Claude's architecture review** independently concluded **cwd is a session location, not config** — different invariants (validation, persistence, immutability-after-mint). rev2 adopted the separation, and the genuinely-better parts of the independent design: the **`SessionCwd` newtype**, the **registry-reuse guard** (per-request cwd must not be a respawn key — a warm process is shared across repos), and the **immutability guard**. Codex's executability review confirmed the seam + caught real blockers (the streaming-workflow path was a rev1 miss; `AgentEntry` is a core-struct ripple; `row_to_task` is positional → all three SELECTs).

**`spawn_cwd` (a host-process-cwd field) was declined as YAGNI — backed by a probe**, not assumption: with `session_cwd=/work` but the container's process cwd (`-w`) `=/elsewhere`, claude edited `/work` and left `/elsewhere` untouched — claude honors the ACP session cwd, not the OS process cwd. So `session_cwd` is the sole "where it works" lever for ACP-compliant agents; `spawn_cwd` would only matter for a hypothetical non-compliant agent, added then. (The concept — host process cwd ≠ session cwd — is documented to prevent the rev1 conflation recurring.)

## Live-gate results (real claude, containerized)

- **DoD-6 (no identical-path hack):** `session_cwd="/work"` distinct from `cwd="/tmp"`, repo mounted at `/work` (not identical-path) → the agent edited the mounted repo, task `Completed`.
- **Per-request targeting:** `a2a-bridge.cwd="/workspace/svc-a"` under a broad parent mount → the agent operated in `svc-a` only (`svc-b` untouched), `Completed`, `tasks.session_cwd` persisted as `/workspace/svc-a`.
- **`allowed_cwd_root`:** `a2a-bridge.cwd="/etc"` (outside `/workspace`) → rejected `invalid request: a2a-bridge.cwd`.

## Consequences

- **One `serve` drives agents across many codebases** — static per-agent `session_cwd` (one serve/agent per repo) OR per-request `a2a-bridge.cwd` (a subdir under a broad mount). The identical-path mount hack is retired.
- **Coverage held:** workspace 90.66%, bridge-core 97.98%, bridge-workflow 92.95% (all floors); full suite + clippy `-D warnings` clean.
- **Hexagonal boundary respected:** the executor stays pure (forwarded context); cwd validity is a type guarantee; the snapshot stays opaque.

## Follow-ons

- **Per-request *mount* templating (Option B)** + **per-task containers** — for hard per-repo filesystem isolation of untrusted work (templated `run` args); deferred. The broad-parent-mount + `session_cwd` covers per-request targeting today.
- **`spawn_cwd`** — only if a non-ACP-compliant agent that honors its process cwd appears.
- The `run_from`/`spawn_detached_workflow` positional-param growth could later fold into a params struct (cosmetic).
