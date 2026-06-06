# `:ro` AcpBackend Container Reaper — Design

**Date:** 2026-06-05
**Status:** Draft (pre dual-review).
**Context lineage:** Bug surfaced while shipping B2b-2 (ADR-0020) — ~15 leaked `a2a-agent-reader`
containers piled up over sessions. First of the B2b-3 lead-in increments (then B2b-3a review-the-diff,
B2b-3b review→tweak loop).

## Goal

Guarantee that the `:ro` containerized ACP agents (the `a2a-agent-reader` review/design agents spawned by
`run-workflow`/`implement` via the `[sandbox]` path) are **removed when their backend is torn down** —
mirroring the `:rw` `ContainerReaper` (B2a). Today they leak.

## Context (the leak)

A sandboxed `kind=acp` agent is spawned as `docker run -i --rm … a2a-agent-reader:latest <cmd>` via
`make_spawn_fn` → `acp_program_argv` → `compose_sandbox` (main.rs / bridge-core), and the `docker run`
**client** process is owned by `Supervised` (`bridge-core/process.rs`: `process_group(0)` +
`kill_on_drop(true)`). `AcpBackend` holds it in `supervised: Arc<StdMutex<Option<Supervised>>>`.

There are **three teardown paths**, and all SIGKILL the client → orphan the `--rm` container (`--rm` only
removes on *clean* client exit):
- `escalate_terminate` (acp_backend.rs:1108) — on cancel (driver:1290, :1409): TAKEs + SIGTERM→SIGKILL.
- `retire` (acp_backend.rs:1456) — lease-drain: TAKEs + terminate.
- `kill_on_drop(true)` — when the `AcpBackend` Arc finally drops (e.g. process/registry teardown): SIGKILL.

The `:rw` path already solved this with a **named container + explicit `docker rm -f <name>`** reaper
(`bridge-container`: `ContainerReaper`, `reap_once`, `reap_argv`, a boot-sweep). The `:ro` path lacks it.

## Decisions (settled with the owner)

1. **Named container + explicit reaper** (NOT a SIGTERM-clean-teardown). Deterministic — cleanup is
   independent of how the client dies; consistent with the `:rw` path; survives a bridge crash (via the
   boot-sweep). Rejected the SIGTERM-then-wait alternative: timing-dependent, no crash recovery, and it
   would touch the shared `Supervised` teardown used by *all* agents.
2. **Reap at all three teardown points, idempotently.** A shared `reaped: Arc<AtomicBool>` (the `:rw`
   `reap_once` pattern) fires `docker rm -f <name>` (detached) exactly once across `escalate_terminate` +
   `retire` + a new `impl Drop for AcpBackend`. `docker rm -f` force-removes (works whether or not the
   client is still up), so reaping is independent of the process-kill order.
3. **Injected `reap_fn`** (mirror B2a's `ReapFn`) so the reaper is unit-testable without Docker — the
   production fn spawns the detached `docker rm -f`; tests inject a recording fn.
4. **Naming** via a pure `compose_sandbox_named` in `bridge-core/sandbox.rs` that splices `--name <name>`
   immediately after `--rm` (the exact splice `compose_container_rw` already does). The token is
   `a2a-ro-<agent_id>-<nonce>` — a **stable `a2a-ro-` prefix** so the boot-sweep can find orphans, plus a
   per-spawn nonce for uniqueness. `reap_argv` (bridge-core) is reused unchanged.
5. **Boot-sweep at startup** (mirror B2a): before any spawn, reap orphaned `a2a-ro-*` containers
   (`docker ps -aq --filter name=^a2a-ro- → rm -f`), recovering leaks from a prior crash.
6. **Only sandboxed `:ro` agents get a name + reaper.** Non-sandboxed (local-process) `kind=acp` agents
   and the `kind=api` agents are untouched. (`:rw` keeps its existing `ContainerReaper`.)
7. **Per-prompt cancel that does NOT kill the process does NOT reap** — the container outlives a single
   workflow node. Reaping is tied to backend teardown (process death), not to a node finishing.

## Architecture

### Naming (pure, bridge-core/sandbox.rs)
```rust
/// PURE. Like compose_sandbox but names the container so a reaper can `docker rm -f` it deterministically.
/// Splices `--name <name>` after the `run -i --rm` prefix (same splice as compose_container_rw).
pub fn compose_sandbox_named(sb: &SandboxConfig, name: &str, cmd: &str, args: &[String])
    -> (String, Vec<String>);
```
plus a pure boot-sweep arg builder:
```rust
/// PURE. (program, argv) to list `:ro` reaper containers for the boot-sweep: ps -aq --filter name=^<prefix>
pub fn ro_sweep_list_argv(runtime: &str, prefix: &str) -> (String, Vec<String>);  // prefix = "a2a-ro-"
```
The per-spawn name is built in the bin: `ro_container_name(agent_id, nonce) -> "a2a-ro-<agent_id>-<nonce>"`
(fs/docker-name-safe; reuse the existing `nonce` helper from implement.rs or a small local one).

### Reaper (bridge-acp)
`AcpConfig` gains an optional field:
```rust
pub struct ContainerReap {
    pub runtime: String,           // "docker" | "podman"
    pub name: String,              // a2a-ro-<agent_id>-<nonce>
    pub reap_fn: ReapFn,           // Arc<dyn Fn(String /*runtime*/, String /*name*/) + Send + Sync>
}
// AcpConfig { …, pub container: Option<ContainerReap> }
```
`AcpBackend` stores `container: Option<ContainerReap>` + `reaped: Arc<AtomicBool>`. A free fn
`reap_once(container: &Option<ContainerReap>, reaped: &Arc<AtomicBool>)` (mirrors B2a) fires the reap at
most once. It is called from:
- `escalate_terminate` — threaded the `container`+`reaped` clones alongside the existing `supervised` clone
  at both call sites (driver:1290, :1409).
- `retire` — after taking/terminating the supervised child.
- **`impl Drop for AcpBackend`** (new) — covers the plain-drop path (normal workflow completion → registry
  drop). Safe + idempotent via `reaped`.

Production `reap_fn` = detached `tokio::process::Command::new(runtime).args(["rm","-f",name])` with a 10s
timeout (B2a's `production_reap_fn`). The in-process `connect` test path leaves `container = None` (no
reaper), exactly as it leaves `supervised = None`.

### Boot-sweep + wiring (bin/a2a-bridge/src/main.rs)
- At `serve` and `run-workflow`/`implement` startup, before the registry spawns anything, run the
  `:ro` boot-sweep once (blocking): `ro_sweep_list_argv` → `docker rm -f` each (best-effort, logged).
- `acp_program_argv` (the sandboxed branch): for an `access=Ro` sandbox, generate the name, use
  `compose_sandbox_named`, and build a `ContainerReap { runtime: sb.runtime(), name, reap_fn }` into the
  `AcpConfig` passed to `AcpBackend::spawn`.

## Component / file boundaries

| Concern | Home | Note |
|---|---|---|
| `compose_sandbox_named` + `ro_sweep_list_argv` (PURE) | `crates/bridge-core/src/sandbox.rs` | golden-tested; reuse `reap_argv` |
| `ContainerReap` + `reap_once` + reap-at-teardown + `impl Drop` | `crates/bridge-acp/src/acp_backend.rs` | injected `reap_fn`; idempotent |
| name gen + named compose + `ContainerReap` build + boot-sweep | `bin/a2a-bridge/src/main.rs` | `acp_program_argv` + startup |

## Testing

- **Unit (no Docker):** `compose_sandbox_named` golden (the `--name` lands right after `--rm`, everything
  else identical to `compose_sandbox`); `ro_sweep_list_argv` golden; `ro_container_name` shape/safety;
  `reap_once` fires the injected `reap_fn` exactly once across repeated calls (idempotent) and not at all
  when `container = None`. The existing `connect`-path AcpBackend tests stay green (`container = None` →
  no reaper, no behavior change).
- **Live gate (Docker, operator-run):** (a) run a `:ro` review workflow (e.g. `spec-review`) → assert NO
  `a2a-ro-*` container remains afterward (`docker ps -aq --filter name=^a2a-ro-` is empty); (b) plant a
  fake orphan `docker run -d --name a2a-ro-planted … sleep 600`, start `serve`/`run-workflow`, assert the
  boot-sweep removed it; (c) confirm the `:rw` `ContainerReaper` path is unaffected (B2a impl-smoke still
  reaps).

## Deferred

- Sharing `reap_once`/`ReapFn` between `bridge-container` (:rw) and `bridge-acp` (:ro) via a common home
  (e.g. bridge-core) — kept duplicated for now to bound blast radius (don't refactor working `:rw` code).
- A periodic/age-based sweep (only boot-time + teardown here).
- The general `Supervised` SIGTERM-clean-exit improvement (orthogonal; not needed once the reaper is
  deterministic).

## Firewall

Designed from the bridge's own seams (`compose_sandbox`/`compose_container_rw` splice, `reap_argv`,
`AcpBackend` teardown, B2a's `ContainerReaper`). Review = a2a-local `codex-review` (gpt-5.5) backstop
ONLY for this increment — the containerized-dogfood spec-review is intentionally skipped here because it
would spawn the exact leaky `:ro` reader containers under fix.
