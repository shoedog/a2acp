# ADR-0021 — The `:ro` AcpBackend Container Reaper

**Date:** 2026-06-05
**Status:** Accepted

**Context lineage:** Bug surfaced while shipping B2b-2 (ADR-0020) — ~15 orphaned `a2a-agent-reader`
containers had piled up over sessions. First of the B2b-3 lead-in increments (then B2b-3a review-the-diff,
B2b-3b review→tweak loop).

---

## Context

The `:ro` containerized review/design agents (`a2a-agent-reader`, spawned by `run-workflow`/`implement` via
the `[sandbox]` path) ran as `docker run -i --rm … <cmd>`, owned by `Supervised` (`process_group(0)` +
`kill_on_drop(true)`). `--rm` removes the container only on a *clean* client exit, but every teardown
(`escalate_terminate` on cancel, `retire` on lease-drain, `kill_on_drop` on Arc drop) **SIGKILLs the client**
→ orphan. Only the `:rw` `ContainerRwBackend` (B2a) had an explicit named-container reaper; the `:ro` path
relied on `--rm` + clean exit, so it leaked.

## Decision

Mirror the `:rw` reaper for the `:ro` path, **extracting the shared primitives to `bridge-core`** so both
paths use one implementation (no drift).

- **Shared reaper in `bridge-core::reaper`** — `ReapFn`/`SweepFn`/`reap_once`/`spawn_detached` (off-runtime-
  safe)/`production_reap_fn`/`production_sweep_fn`, extracted from `bridge-container`; the `:rw`
  `ContainerReaper` now imports them. `reap_argv` stays in `bridge-core::sandbox`.
- **Named container** `a2a-ro-<owner>-<nonce>` via a pure `compose_sandbox_named` (the `--name` splice
  `compose_container_rw` already uses). `owner = container_owner(config_path, mount, agent_id)` (a hex hash
  → Docker-name-safe even though `AgentId` permits spaces/slashes; the raw id never enters the name).
- **Reap at four teardown sites, idempotently** (shared `reaped: AtomicBool`): the spawn/handshake-failure
  path (the container is up but no backend exists to reap from — mirrors `:rw`), `escalate_terminate`
  (cancel), `retire` (lease-drain), and a new `impl Drop for AcpBackend`. `forget_session` is per-node, not
  teardown. Reaper attached via an optional `ContainerReap` on `AcpConfig`; `reap_fn` is injected so the
  logic is Docker-free unit-testable; `ContainerReap` has a manual `Debug` so `AcpConfig` keeps deriving it.
- **Owner-scoped boot-sweep** at startup (reaps `a2a-ro-<owner>-` for each `:ro` agent, off the SNAPSHOT so
  the normalized mount + canonical config_path match the spawn owner) — recovers a prior crash's orphans
  WITHOUT touching a concurrent bridge's containers (different config → different owner).
- **Both spawn factories** (`make_spawn_fn` + the `serve` closure) wire the name+reaper via a shared
  `acp_spawn_inputs` helper, so they can't diverge. Config-path canonicalized at all three startups so the
  sweep owner == the spawn owner (this also fixed a latent `:rw` owner inconsistency — `serve` canonicalized
  but `run-workflow`/`implement` did not).

## Components

| Concern | Home |
|---|---|
| `ReapFn`/`SweepFn`/`reap_once`/`spawn_detached`/`production_{reap,sweep}_fn` (shared) | `crates/bridge-core/src/reaper.rs` (new) |
| `compose_sandbox_named` / `ro_container_name` / `ro_sweep_filter_argv` (pure) | `crates/bridge-core/src/sandbox.rs` |
| `:rw` `ContainerReaper` imports the shared primitives | `crates/bridge-container/src/lib.rs` |
| `ContainerReap` + 4-site idempotent reap + `impl Drop` | `crates/bridge-acp/src/acp_backend.rs` |
| `acp_spawn_inputs` (both factories) + boot-sweep + the one-shot END-sweep guard | `bin/a2a-bridge/src/main.rs` |

## Dual-review + live-gate findings

Firewalled a2a-local `codex-review` (the containerized dogfood was skipped — it would spawn the leaky `:ro`
containers under fix). Spec review caught the **4th reap site** (spawn-failure), the **global-boot-sweep
hazard** (a global `a2a-ro-` prefix would kill a concurrent bridge's live containers → owner-scoping), the
Docker-name safety (hash the owner), the manual `Debug`, both factories, and off-runtime-safe `Drop`. Plan
review caught two **missed struct literals** (bridge-container's `AcpConfig`; the `retire_is_idempotent`
test), the **unused-imports clippy fail** after extraction, the **owner-divergence** (normalized snapshot
mount + canonical config_path), and made the coverage honest (a Docker-free spawn-failure test).

**Live-gate finding (the dogfood earned its keep):** the per-backend detached `Drop` reap is correct for
long-running `serve` (the runtime stays alive) but **races process exit** on a one-shot `run-workflow`/
`implement` — the runtime dies before the detached `docker rm -f` runs. Fix: a synchronous **RAII
end-sweep** (`RoSweepGuard`) that reaps this run's `:ro` containers on any return path. (`serve` keeps the
boot-sweep + retire reaps.)

## Validation

- Unit (Docker-free): `reap_once` idempotency + `spawn_detached` off-runtime (bridge-core); `compose_sandbox_named`/`ro_container_name`/`ro_sweep_filter_argv` golden; `AcpBackend` 4-site idempotent reap + the **spawn-handshake-failure** reap (`/bin/cat` + short handshake → asserts the injected reap_fn fired). bridge-container's reaper tests stay green post-extraction.
- Live gate (Docker, dogfooded on this repo): (1) a `:ro` `spec-review` → `a2a-ro-*` reaches **0 within ~2s** (the residual lag is Docker Desktop's async daemon removal after `docker rm -f`, not a leak — vs the prior hours-long pileup); (2) owner-scoped boot-sweep — a planted this-owner orphan is reaped, a planted other-owner orphan is **left** (concurrency-safe); (3) `:rw` `impl-smoke` still reaps (extraction safe).
- Coverage (floors per ci.yml): bridge-core **95.65%** (sandbox.rs 100%; reaper.rs's `production_{reap,sweep}_fn` are live-only/Docker-spawning, uncovered before the move too), bridge-acp **94.16%** — both ≥90; workspace ≥85; clippy `-D warnings` clean.

## Deferred

- Sharing the boot-sweep with `:rw` (it already has its own); an age/label-based sweep + a same-config
  concurrency lock (boot + teardown + end-sweep only; same-config concurrent bridges share owners — B2a's
  known limitation). The general `Supervised` SIGTERM-clean-exit improvement (orthogonal once reaping is
  deterministic).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
