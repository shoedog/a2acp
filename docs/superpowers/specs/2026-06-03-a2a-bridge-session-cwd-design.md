# Session cwd / Per-Request Repo Targeting — Design

**Date:** 2026-06-03
**Status:** Draft (brainstormed; pending user review → plan)

**Goal:** Decouple the ACP **session working directory** from the host **process-spawn** directory, and let a single `message/send` **target a specific repo per request** (via metadata) — so one `serve` can drive agents against many codebases (containerized or host) without the identical-path mount hack, and review agents can stay container-free.

---

## Background / problem

The bridge currently uses **one `cwd`** for two different things:
1. the **host child process** working directory (`bridge-core/process.rs` `current_dir`), and
2. the **ACP `session/new` cwd** (`bridge-acp/acp_backend.rs` `new_session_request(cwd)`), required absolute per ACP §11A.

For a **containerized agent** these differ: the host child is `docker/podman run …` (runs anywhere), but the agent operates at the *in-container* repo path. The live container probe worked only via an **identical-path mount** (`-v repo:repo` + matching `cwd`) — a hack that collapses the two values. And targeting a **different repo per request** is impossible today: `cwd` is fixed per agent-entry, so multi-repo use needs one serve/agent per repo.

This increment fixes both: a distinct `session_cwd`, plus a per-request cwd override.

## Architecture

Three pieces; the first is trivial, the rest build per-request on top.

### A. Decouple `session_cwd` (config)
Add `session_cwd: Option<String>` to the agent config (`AgentEntryToml` → `AgentEntry`). The host child keeps using `cwd`; **`session/new` uses `session_cwd` when set**, else falls back to `cwd`, else `"."` (backward compatible — existing configs unchanged). This alone removes the identical-path hack: a containerized agent sets `cwd` for the host `run` and `session_cwd` to the in-container repo path.

### B. Per-request cwd (the substantive part)
A `message/send` / `message/stream` may carry **`a2a-bridge.cwd`** (absolute path) in `message.metadata` (the same metadata channel that already carries `a2a-bridge.skill`). When present and valid, it overrides the session cwd **for that task only**.

**Resolution chain for a session's cwd** (first that applies):
`request a2a-bridge.cwd` → agent `session_cwd` → agent `cwd` → `"."`.

This applies to **both dispatch paths**:
- **Single-agent** (`Local` route, sync): the routed request carries the resolved cwd; the dispatch's `session/new` uses it.
- **Workflow** (detached): the submit's `a2a-bridge.cwd` is captured at submit, **persisted with the task**, and applied to **every node's** agent session for that task. It threads through the executor as an *opaque forwarded dispatch parameter* (alongside `run_id`/`cancel`) — the executor's topo/scheduling logic does not interpret it, preserving executor purity. Concretely `run_from(graph, input, run_id, cancel, seed, session_cwd)` forwards `session_cwd` to `run_node` → the backend session-creation; the graph and seed are untouched.

### C. Persistence for resume (W3b interaction)
Because a detached workflow can crash and **resume** (ADR-0011), the per-request cwd must survive a restart, or resumed nodes would get the wrong directory. Persist it as an **additive `tasks.session_cwd` column** (same pattern as `input` / `workflow_spec_json`), captured at detached submit and restored into `run_from` on boot resume.

## Components

- **`bin/a2a-bridge/src/config.rs`:** `AgentEntryToml.session_cwd: Option<String>` (+ optional `allowed_cwd_root` — see Security).
- **`bridge-registry` / `AgentEntry`:** carry `session_cwd`; the spawn fn (`main.rs`) sets the host child cwd from `cwd` and the `AcpBackend` session-cwd config from `session_cwd` (fallback to `cwd`).
- **`bridge-acp/acp_backend.rs`:** `new_session_request` already takes a cwd; the change is *which* value flows in — a per-request override when present, else the static config. The session-creation seam (`ensure_session` / the per-session `configure_session` that already applies model/mode) is where the per-request cwd is applied at mint time.
- **`bridge-a2a-inbound/src/server.rs`:** extract `a2a-bridge.cwd` from `message.metadata` (mirroring `a2a-bridge.skill` extraction); validate (Security); thread it into single-agent dispatch and into the detached workflow path (persist + pass to `spawn_detached_workflow` → `run_from`).
- **`bridge-workflow/src/executor.rs`:** `run_from`/`run_node` gain an opaque `session_cwd: Option<String>` forwarded to the backend dispatch. No change to graph/seed/scheduling.
- **`bridge-core/src/task_store.rs` + `bridge-store/src/sqlite.rs`:** `TaskRecord.session_cwd: Option<String>` + an additive `tasks.session_cwd` column (idempotent migration, same mechanism W3b used); restored on `working_tasks()` and passed into resume.

## Data flow

- **Single-agent:** `message/send {metadata.a2a-bridge.cwd}` → validate → routed request carries cwd → `Local` dispatch → `session/new(cwd)` → agent operates there.
- **Workflow submit:** `message/send {skill, a2a-bridge.cwd}` → validate → persist `TaskRecord{… session_cwd}` → `spawn_detached_workflow(… session_cwd)` → `run_from(… session_cwd)` → each `run_node` dispatch creates the node-agent session with `session_cwd`.
- **Resume:** boot reads `tasks.session_cwd` → `run_from(… session_cwd)` for the resumed run → un-checkpointed nodes re-run in the correct directory.

## Security / operating guidance

**`allowed_cwd_root` (defensive).** Optional config (global or per-agent): if set, a per-request `a2a-bridge.cwd` must canonicalize to a path **under** the root, else the request is rejected (`InvalidRequest`) before any dispatch. This prevents a request from pointing an agent at `/etc`, `~/.ssh`, or outside the intended workspace. The cwd must also be **absolute** (ACP §11A); a relative or non-existent cwd is rejected.

**Per-role containerization (operational; NOT bridge code).** The bridge change is cwd-routing only; how you *run* each agent is config:
- **Inlined-context, tools-off reviewers/planners/architects** (today's review/spec/plan workflows) → **host, no container** — zero disk read/write, lightest. The per-request cwd is irrelevant (context is inlined).
- **Tool-using readers** (repo exploration) → containerized with a **read-only** mount (`-v repo:repo:ro`) — confine reads + egress; no write risk.
- **Editors / dev agents** → containerized with a **writable** target-repo mount + egress lockdown.

**Mounting (operational).** Mount the **code tree** (`~/code` or a dedicated workspace root), **never `~`** — the home dir holds read-secrets (`~/.ssh`, tokens, `~/.aws`) that a read-only mount still exposes and any egress can exfiltrate. Mount each agent's **skills/config subdirs read-only** (`~/.claude`/`~/.codex`/`~/.kiro` skills/plugins/settings), **not** the whole dir (which mixes in credentials/history/sessions); inject credentials separately. Sibling-repo *visibility* under a broad code-tree mount is acceptable; *writes* are confined by `:ro` / by mounting only the target repo writable.

## Error handling
- Per-request cwd absent → use the config fallback chain (no error).
- Per-request cwd present but relative / non-absolute, or outside `allowed_cwd_root` → reject the request with `InvalidRequest` + reason, before dispatch / before minting a task.
- A containerized agent whose `session_cwd` is not reachable inside the container (operator mis-mounted) → `session/new` fails in-container → the existing backend-error path fails the node/task (no new handling; surfaced via the existing agent-error → Failed flow).

## Definition of Done
1. `session_cwd` config field; `session/new` uses it (fallback `cwd`→`"."`); existing configs behave identically. Test: a config with `session_cwd` distinct from `cwd` mints a session at `session_cwd`.
2. `a2a-bridge.cwd` metadata extracted + validated (absolute; `allowed_cwd_root` if set). Tests: valid override reaches `session/new`; relative/escape rejected with `InvalidRequest`.
3. Single-agent dispatch honors the per-request cwd. Test (live or fake-backend) asserting the session cwd.
4. Workflow dispatch applies the per-request cwd to all nodes; executor purity preserved (forwarded param only). Test: a multi-node workflow's nodes mint sessions at the request cwd.
5. `tasks.session_cwd` persisted at submit + restored on resume; idempotent migration of pre-existing DBs. Test: a resumed workflow re-runs pending nodes at the persisted cwd.
6. The identical-path mount hack is no longer required — a containerized agent works with `cwd` (host) ≠ `session_cwd` (container). Validate against the container probe setup (distinct paths).
7. fmt/clippy/coverage floors hold; ADR recorded.

## Out of scope (explicit)
- **Per-request *mount* templating (Option B):** dynamically mounting only the requested repo per task (templated `run` args + per-task containers) — deferred; broad code-tree mount (Option A) + `session_cwd` covers per-request targeting now. Build B when hard per-repo filesystem isolation is required.
- **Egress-controlled network:** the real confidentiality control for containerized agents — a separate piece (own design/ops), not this bridge increment.
- A general "container backend" that orchestrates container lifecycle (vs `run -i` as the agent cmd) — not needed; the process/ACP path suffices.

## Firewall
Designed from the bridge's own ports (registry/AcpBackend/TaskStore/executor) + ACP §11A session semantics + A2A message metadata; the `a2a-local-bridge` PoC did not inform it.
