# ADR-0025 — Concurrency-safe containerized runs (Increment A)

**Date:** 2026-06-08
**Status:** Accepted

**Builds on:** B2a (`ContainerRwBackend`, ADR-0018), the `:ro` reaper (ADR-0021), the warm `implement`
session (ADR-0024), and `session_cwd` / per-request repo (ADR-0014).

---

## Context

Before this increment, every managed container was named `a2a-{role}-{owner}-{seq}`, where
`owner = hash(config_path, mount, agent_id)` and `seq` restarted at 0 each process. Crash-orphan recovery
was an **owner-name boot sweep** (`docker ps --filter name=a2a-{rw,ro}-{owner}-` → `rm -f`), run at backend
construction (`:rw`) / command start (`:ro`).

Two processes that shared a config + agent — the *same* repo twice, or two different repos — collided:

1. **Name clash.** Both minted `a2a-rw-{owner}-0` → the second `docker run --name` hard-failed.
2. **Cross-reap.** The owner-name boot sweep could not tell a *crashed* orphan from a *live peer's*
   container (same owner prefix), so starting a second run `rm -f`'d the first run's live containers.

So the only safe way to parallelize was a distinct config file per project (see
[[onboarding-usage-hardening-shipped]]). The goal of this increment: make concurrent containerized runs
(same OR different repo, ONE shared config) safe — no clash, no cross-reap — with crash-orphan recovery, an
operator visibility/cleanup surface, and **no database**.

## Decision

**Docker labels + a per-process OS `flock` lease ARE the registry** (no DB).

- **Per-process run identity.** Each process mints `instance_id = "{pid}-{nonce}"` (the label `a2a.run`),
  deliberately distinct from the executor/task `run_id`. It is stamped into every managed container's NAME
  (`a2a-{role}-{owner}-{instance_id}-{tail}`), so a same-owner concurrent run can no longer clash.
- **A full managed label set** (`run_identity::ContainerLabels`) on every `:rw`/`:ro` container:
  `a2a.managed=1`, `a2a.role/kind/agent/owner/run/host/lease/start`, plus display-only `a2a.repo`/`a2a.cwd`.
  Stamped at the compose layer so both roles inherit it (`compose_sandbox` splices the `--label`s).
- **`flock` lease = liveness.** A process holds an exclusive `flock` on `<lease_dir>/<instance_id>.lock`
  for its whole life; the OS releases it when the process dies — clean OR crash. A sweeper that can
  *acquire* the lock ⇒ the owner is gone. This is PID-reuse-, clock-drift-, and reboot-safe (unlike probing
  PID start-times) and needs **no new dep** (`libc::flock`). The `a2a.host` label gates cross-machine: a
  different host ⇒ `Unknown` ⇒ spared.
- **Pure `classify(labels, my_host, probe) -> {Alive, Dead, Unknown}`**, fail-safe toward *sparing*:
  another host, a missing `a2a.host`, or an absent/unreadable lease all yield `Unknown` (treated as Alive);
  only same-host **and** a free lock yields `Dead`. Only `Dead` permits an automatic reap.
- **Three reap scopes:**
  1. **Run-scoped END-sweep** — `rm -f` by `label=a2a.run=<instance_id>` on command exit (one-shots only;
     a single `RunEndGuard`, label-scoped so a concurrent run is never touched).
  2. **Before-first-use crash recovery** — `classify_sweep` over each owner's MANAGED containers, **Dead
     only**, at every entry point (implement / run-workflow start, serve startup AND hot-reload), over the
     UNION of `:rw` + `:ro` owners. Replaces the construction/start boot sweeps.
  3. The unchanged per-turn specific-name reaper + warm `retire` (ADR-0021/0024).
- **`containers list|reap`** — the operator surface over the label/lease registry. `list` classifies every
  managed container (alive/dead/unknown + stale + age), scoped to *this config's* owners by default
  (`--all` = host-wide), plus a list-only pass for legacy (pre-A, unlabeled) `a2a-{ro,rw}-*` names. `reap`
  defaults to this config's owners, **Dead only**; `--all-dead` widens scope, `--run`/`--owner` pin one
  (still Dead-only), `--stale [--older-than <dur>]` reaps Alive-but-idle (no output within the window), and
  `--force <name>` reaps exactly that container regardless of state (the only Alive/legacy override).
- **The atomic-flip constraint.** The `run_id`-in-name change and the boot-sweep → `classify_sweep` flip
  MUST land in ONE commit. Split either way reintroduces a failure: old-scheme orphans without the run-id
  segment would clash a peer's first `docker run --name`, or the surviving owner-name boot sweep would
  cross-reap. (Realized as Slice S3, commit `29b97e3`.)

## Consequences

- **Concurrent containerized runs (same OR different repo, one shared config) are safe.** The
  [[onboarding-usage-hardening-shipped]] "distinct config per project" rule is **lifted** — AGENTS.md
  updated accordingly.
- **Crash orphans recover automatically** (Dead-only, before first use) and are visible + manually reapable
  via `containers`. A live peer's containers (held lease) are never reaped.
- **No DB, no new dependency.** `libc::flock` + docker labels only.
- **The reaper's docker shell-out** (`run_scoped_reap` / `classify_sweep` / `is_stale`) is exercised by the
  live gate, not unit tests; the *pure* decision (`classify`) and the `containers` pure cores
  (record parse / reap plan / row format) are unit-tested (`run_identity` 99%, `sandbox` 100%, the
  `containers` module fully covered). `cargo llvm-cov`: bridge-core 93.4% line, workspace 88.5% line.
- **serve has no END-sweep** — it's long-running, so per-backend `retire` (runtime alive) + the next run's
  before-first-use recovery cover any leftover. The serve process holds its lease for its whole life.
- **Lease files** live under `$A2A_LEASE_DIR` (else `$HOME/.a2a-bridge/leases`); a clean exit removes the
  file, a crash leaves it with a free lock (the recovery signal).

## Deferred to Increment B

A SQLite run registry; ACP-stream-driven `last_activity` (vs the `docker logs --since` staleness probe);
*automatic* staleness reaping (A only reports + offers manual `--stale`); `--resume` of a long warm run at
the ~1h cache horizon; serve per-request isolation; non-container path coverage; same-target write locks.
Legacy (pre-A unlabeled) containers stay list-only (reap via `--force <name>`).
