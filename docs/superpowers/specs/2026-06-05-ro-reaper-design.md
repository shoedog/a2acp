# `:ro` AcpBackend Container Reaper — Design

**Date:** 2026-06-05
**Status:** Draft (rev2, post codex spec-review). Folds the firewalled a2a-local `codex-review` (needs-
changes): a 4th reap site (spawn/handshake failure), owner-scoped naming + boot-sweep (not a global
prefix), Docker-name-safe token, `ContainerReap` manual `Debug`, both spawn factories, off-runtime-safe
`Drop`, substring (not anchored) filter — and the owner-confirmed **extract-shared-reaper-to-bridge-core**.
**Context lineage:** Bug surfaced in B2b-2 (ADR-0020). First of the B2b-3 lead-in increments.

## Goal

Guarantee the `:ro` containerized ACP agents (`a2a-agent-reader` review/design agents spawned via the
`[sandbox]` path) are **removed when their backend is torn down OR fails to spawn** — mirroring (and now
sharing the primitives of) the `:rw` `ContainerReaper` (B2a). Today they leak (~15 piled up in B2b-2).

## Context (the leak)

A sandboxed `kind=acp` agent runs as `docker run -i --rm … a2a-agent-reader:latest <cmd>` (main.rs →
`acp_program_argv` → `compose_sandbox`); the `docker run` **client** is owned by `Supervised`
(`process_group(0)` + `kill_on_drop(true)`), held in `AcpBackend.supervised`. `--rm` removes the container
only on *clean* client exit, but every teardown SIGKILLs the client → orphan. The `:rw` path solved this
with a named container + explicit `docker rm -f <name>`; `:ro` lacks it.

## Decisions (settled with the owner)

1. **Named container + explicit reaper** (not SIGTERM-clean-teardown). Deterministic; consistent with `:rw`;
   crash-recoverable via the boot-sweep.
2. **Extract the shared reaper primitives to `bridge-core`** [owner-confirmed]. A new `bridge-core/src/
   reaper.rs` owns `ReapFn`, `reap_once`, `spawn_detached` (off-runtime-safe), and `production_reap_fn`;
   `reap_argv` stays in `bridge-core/sandbox.rs`. `bridge-container`'s `:rw` `ContainerReaper` switches to
   import them (mechanical), and the new `:ro` reaper uses them — no drift, the hard-won off-runtime /
   idempotency / spawn-failure correctness lives once. (`container_owner` is already shared in the bin.)
3. **Reap at FOUR sites, idempotently** (a shared `reaped: Arc<AtomicBool>` via `reap_once`):
   - **spawn/handshake failure** — in `AcpBackend::spawn`, if `connect` errors *after* the `Supervised`
     child (the `docker run`) started (e.g. handshake timeout, acp_backend.rs:709), reap the container
     before returning `Err` (mirrors `:rw` lib.rs:194). One-shot on the error path (no backend exists yet).
   - **`escalate_terminate`** (cancel; acp_backend.rs:1290, :1409) — reap after taking/terminating supervised.
   - **`retire`** (registry lease-drain → registry.rs:275) — reap after terminate.
   - **`impl Drop for AcpBackend`** (NEW) — the plain-drop path (normal completion → registry drop).
   `forget_session` (acp_backend.rs:1438) is per-node, NOT teardown → no reap (the container outlives a node).
   `docker rm -f` force-removes regardless of client state, so reap order vs the process-kill is irrelevant.
4. **Owner-scoped, Docker-name-safe naming.** name = `a2a-ro-<owner>-<nonce>` where
   `owner = container_owner(config_path, sb.mount, agent_id)` — the EXISTING bin fn (main.rs:121), a hex
   hash (Docker-name-safe even though `AgentId` permits spaces/slashes; the raw id never enters the name).
   `nonce` per spawn for uniqueness. A pure `compose_sandbox_named` splices `--name <name>` after `run -i
   --rm` (same splice as `compose_container_rw`).
5. **Owner-scoped boot-sweep** (not a global `a2a-ro-` prefix — that would kill a *concurrent* bridge's live
   containers). At startup, for each `access=Ro` sandboxed agent in the config, compute its owner and reap
   `name=a2a-ro-<owner>-` (Docker's `name` filter is **substring**, not anchored — owner-scoping makes the
   substring specific). Two bridges with the *same config* share owners → same B2a limitation (documented;
   the warm-pool/concurrency slice's concern).
6. **Injected `reap_fn`** (the shared `ReapFn`) so the reaper is Docker-free unit-testable; production fn =
   detached `docker rm -f` with a 10s timeout. `ContainerReap` gets a **manual `Debug`** (redacts the fn)
   so `AcpConfig` keeps `#[derive(Debug)]`.
7. **Wire BOTH spawn factories** — `make_spawn_fn` (main.rs:162) AND the `serve` closure (main.rs:1279).
   `acp_program_argv` returns the chosen name so both sites build the `ContainerReap`.
8. **Only sandboxed `access=Ro` `kind=acp` agents** get a name+reaper. Local-process acp + `kind=api` +
   `:rw` (its own `ContainerReaper`) untouched. The `connect` test path leaves `container = None`.

## Architecture

### bridge-core/src/reaper.rs (NEW — shared by :rw and :ro)
```rust
pub type ReapFn = std::sync::Arc<dyn Fn(String /*runtime*/, String /*name*/) + Send + Sync>;
/// Fire `reap_fn(runtime, name)` at most once across calls (idempotent across the 4 sites).
pub fn reap_once(reaped: &Arc<AtomicBool>, runtime: &str, name: &str, reap_fn: &ReapFn);
/// Off-runtime-safe detached spawn: Handle::try_current() else a std::thread w/ a tiny runtime.
pub fn spawn_detached<F: std::future::Future<Output = ()> + Send + 'static>(fut: F);
/// Production reaper: detached `<runtime> rm -f <name>` (reap_argv) with a 10s timeout.
pub fn production_reap_fn() -> ReapFn;
```
`bridge-container` deletes its local copies (lib.rs:306/359/410) and imports these; its `ContainerReaper`
struct + `wrap_with_reaper` + boot-sweep stay (re-tested).

### Naming (pure, bridge-core/sandbox.rs)
```rust
pub fn compose_sandbox_named(sb: &SandboxConfig, name: &str, cmd: &str, args: &[String])
    -> (String, Vec<String>);          // --name <name> spliced after `run -i --rm`
pub fn ro_sweep_filter_argv(runtime: &str, owner: &str) -> (String, Vec<String>);
                                       // ps -aq --filter name=a2a-ro-<owner>-   (substring; owner-scoped)
```
Name built in the bin: `ro_container_name(owner, nonce) -> "a2a-ro-<owner>-<nonce>"`.

### Reaper (bridge-acp/src/acp_backend.rs)
`AcpConfig` gains `pub container: Option<ContainerReap>` where
```rust
pub struct ContainerReap { pub runtime: String, pub name: String, pub reap_fn: ReapFn }
impl std::fmt::Debug for ContainerReap { /* redact reap_fn */ }
```
`AcpBackend` stores `container: Option<ContainerReap>` + `reaped: Arc<AtomicBool>`. `spawn` reaps on the
connect-error path; `escalate_terminate` (threaded the `container`+`reaped` clones at both call sites),
`retire`, and `impl Drop` call `reap_once`. All detach via the shared `spawn_detached` (off-runtime-safe).

### Boot-sweep + wiring (bin/a2a-bridge/src/main.rs)
- At `serve` / `run-workflow` / `implement` startup, before the registry spawns: for each `access=Ro`
  sandboxed agent, `ro_sweep_filter_argv(runtime, owner)` → `docker rm -f` each id (best-effort, logged).
- `acp_program_argv` (Ro-sandbox branch): compute `owner = container_owner(config_path, sb.mount, id)` +
  `nonce` → name; use `compose_sandbox_named`; return the name. Both spawn factories build
  `ContainerReap { runtime: sb.runtime(), name, reap_fn: production_reap_fn() }` into `AcpConfig`.

## Component / file boundaries

| Concern | Home | Note |
|---|---|---|
| `ReapFn`/`reap_once`/`spawn_detached`/`production_reap_fn` (shared) | `crates/bridge-core/src/reaper.rs` (NEW) | extracted from bridge-container |
| `compose_sandbox_named` + `ro_sweep_filter_argv` (PURE) | `crates/bridge-core/src/sandbox.rs` | golden-tested |
| `:rw` `ContainerReaper` switches to import the shared primitives | `crates/bridge-container/src/lib.rs` | mechanical; re-gated |
| `ContainerReap` + `Debug` + 4-site `reap_once` + `impl Drop` | `crates/bridge-acp/src/acp_backend.rs` | injected `reap_fn`; idempotent |
| owner+name+named-compose+`ContainerReap` build (BOTH factories) + boot-sweep | `bin/a2a-bridge/src/main.rs` | reuse `container_owner` |

## Testing

- **Unit (no Docker):** `compose_sandbox_named` golden (`--name` right after `--rm`, else identical);
  `ro_sweep_filter_argv` golden; `ro_container_name` shape; `reap_once` fires the injected fn exactly once
  across repeated calls AND across simulated escalate+retire+Drop, never when `container=None`;
  `spawn_detached` off-runtime (call from a plain thread, no panic); `ContainerReap` `Debug` redaction;
  owner-name is Docker-safe for an `agent_id` with spaces/slashes (hash → hex). Existing `connect`-path
  AcpBackend tests stay green (`container=None`). bridge-container's reaper tests stay green post-extraction.
- **Live gate (Docker):** (a) run `spec-review` → NO `a2a-ro-*` remains; (b) handshake-fail a spawn (bad
  image/cmd) → its container is reaped (4th site); (c) plant `a2a-ro-<this-owner>-x` orphan → boot-sweep
  removes it, and a *different-owner* `a2a-ro-<other>-y` is LEFT (scoping proof); (d) **re-gate `:rw`**:
  B2a `impl-smoke` still reaps (the extraction didn't break `:rw`).

## Deferred

- Age/label-based sweep + a same-config concurrency lock (only boot-time + teardown here; same-config
  concurrent bridges share owners = B2a's known limitation).
- The general `Supervised` SIGTERM-clean-exit improvement (orthogonal once reaping is deterministic).

## Firewall

Designed from the bridge's own seams. Review = a2a-local `codex-review` (gpt-5.5) backstop ONLY — the
containerized-dogfood spec/plan-review is intentionally skipped for THIS increment (it would spawn the exact
leaky `:ro` containers under fix). Resumes once the reaper lands.
