# Concurrency-safe containerized runs — Increment A (design, rev3)

**Status:** Proposed (2026-06-07). Brainstormed + scoped with the owner; clean-room cross-check + dual
spec-review folded (rev3). Decisions: **flock lease as the primary liveness mechanism**; **serve-request
isolation deferred to B**. Pending the owner's spec review, then planning.

**Goal:** Let multiple containerized `a2a-bridge` runs run **concurrently against the same OR different repos
using one shared config** without clashing or cross-reaping each other's containers; **recover** containers
orphaned by a **crashed** run; give the operator **visibility** + manual cleanup; surface **stuck-but-alive**
runs — all with **no database** (docker labels + an OS file-lock are the registry).

**Non-goals (→ Increment B):** SQLite run registry; precise ACP-stream `last_activity`; *automatic* staleness
reaping; `--resume` lineage; cross-process coordination; non-container (`api`/ollama) coverage; **per-request
isolation inside one `serve` process** (A protects all of a live `serve` process's containers as a unit;
isolating/reaping one request among live siblings is B).

---

## Problem

Containers are named + reaped by an **owner** = `hash(config_path, mount, agent_id)` (`container_owner`,
`main.rs`). Same-config runs share an owner, which breaks concurrency two ways:

1. **Name clash (`:rw`).** `a2a-rw-{owner}-{turn_seq}` starts `turn_seq` at 0 → two same-owner runs both grab
   `a2a-rw-{owner}-0` → the second `docker run --name` fails. (`:ro` already dodges this via a nonce.)
2. **Owner-wide boot-sweep cross-reap (`:rw` AND `:ro`).** Each backend's startup sweep reaps `a2a-{rw,ro}-
   {owner}-*` to clean a *previous crashed* run's orphans — but with a shared owner it **kills a concurrent
   live peer's** containers. `implement` uses a `:rw` impl agent *and* `:ro` review readers, so both must be
   fixed.

The owner-wide boot-sweep is also the *only* automatic crash-orphan recovery, and it can't distinguish a
crashed orphan from a live peer.

---

## Decisions

### D1 — Identity + labels (docker labels + a lease file ARE the registry; no DB)

- **`owner`** (= `hash(config_path, mount, agent_id)`, unchanged) stays the **per-container grouping key**.
  A process has **many** owners (one per sandboxed agent — `ro_sweep_targets`/`rw_sweep_targets` already
  enumerate them); owner is computed **per spawn**, as today.
- **`run_id`** = a UUID minted once per **top-level run** (a one-shot `implement`/`run-workflow` *is* one run
  = one process; a `serve` process is one run for A's purposes — see D7). This is the run-identity, distinct
  from the executor/task `run_id` execution-id (call the new field `instance_id` internally; the label is
  `a2a.run`).
- **lease** = a host-local file (`<runtime-dir>/a2a-bridge/leases/<run_id>.lock`) the process holds an
  exclusive `flock` on for its whole life (D3).
- **host** = a stable host id (`a2a.host`) so a sweep never reaps containers a *different* host owns.

Every managed container (`:rw` and `:ro`) is launched with:
```
--label a2a.managed=1
--label a2a.role={rw|ro}   --label a2a.kind={warm|perturn|oneshot}   --label a2a.agent=<agent id>
--label a2a.owner=<owner>  --label a2a.run=<run_id>  --label a2a.host=<host id>  --label a2a.lease=<lease path>
--label a2a.repo=<repo>    --label a2a.cwd=<cwd>     --label a2a.start=<rfc3339>     # display-only, sanitized
```
Identity values are hashes/UUIDs/paths (docker-label-safe); `repo`/`cwd` are display-only, sanitized
(printable, length-capped, omitted if N/A). Query surface: `docker ps -a --filter label=a2a.managed=1
--format '{{.Label "a2a.run"}}\t{{.Label "a2a.host"}}\t{{.Label "a2a.lease"}}\t…'`.

### D2 — `run_id` in the name (kills the clash); ships atomically with D4's flip

```
a2a-rw-<owner>-<run_id>-<seq>        a2a-ro-<owner>-<run_id>-<nonce>
```
Two runs share `owner` but differ in `run_id` → never collide on a `--name`. Grouping/reaping is by **label**
(`a2a.owner`/`a2a.run`), not name substring; the existing per-turn/`retire` reapers keep reaping a specific
known name (now carrying `run_id`).

> **HARD ORDERING CONSTRAINT.** The `:rw` `run_id`-in-name change (D2) and the boot-sweep flip (D4.2) MUST
> ship in the **same slice, name-first** (S3). Today a second run's boot-sweep reaps the first's
> `a2a-rw-{owner}-0` to *free the name* for its own mint; removing that reap before each run has a unique
> name makes the second `docker run --name` hard-fail. One event, one atomic change. (Also retires
> `new_with_hooks`'s `sweep_fn`/sweep-before-mint, which exists only because `turn_seq` restarts at 0.)

### D3 — Liveness = a `flock` lease (clock-/PID-/reboot-/host-safe)

Each run holds an exclusive **`flock`** on its `a2a.lease` file for its lifetime. `classify(labels, host)`:

- `a2a.host` ≠ my host id → **Unknown ⇒ Alive (spare)** — never reap another machine's containers.
- else try a **non-blocking exclusive `flock`** on `a2a.lease`:
  - lock is **held** (acquire would block) → owner **Alive** → spare.
  - lock is **free** (acquired) → owner **Dead** → reap (then release + remove the lease). *This is the
    crash path:* the OS keeps the lease file and releases the lock when the owner dies.
  - lease file **absent** → **Unknown ⇒ Alive (spare)** — abnormal (a live run creates + holds its lease
    before any container exists; the END-sweep removes the lease only after reaping its containers), so don't
    risk wrong-killing a just-starting peer; surfaced in `list` for manual `--force`.
- any probe error (can't stat/open) → **Unknown ⇒ Alive (spare)**.

Why flock over PID+start+boot (the dual review's BLOCKER): the OS releases the lock automatically on crash —
**immune to clock drift (macOS `kern.boottime` wrong-kill), PID reuse, reboot**, and needs no `/proc`/`ps`/
`sysctl`; it's cross-platform. Dep: `fs2` (already in `bridge-store`) or `libc::flock` (workspace has
`libc`). Caveat: a lease dir on NFS is unreliable — out of scope (single host, local dir).

**Lease lifecycle:** create + `flock` right after building the `RunHandle`; hold for the process; on clean
exit, the END-sweep (D4.1) reaps `a2a.run` containers then removes the lease. On crash: OS drops the lock
(file persists) → a later same-host sweep acquires it → reaps the orphans + removes the lease.

### D4 — Reap scopes (all label-scoped; replace the owner-wide sweep)

1. **This run's cleanup** (END-sweep `RwSweepGuard`/`RoSweepGuard`): reap `--filter label=a2a.run=<my run_id>`
   **unconditionally** (cleaning my own live containers on exit), then remove my lease. A peer's `a2a.run`
   differs → never cross-reaps.
2. **Crash-orphan recovery sweep** — runs **before first use of each owner** (covers `serve` lazy-spawn /
   hot-reload, not just startup; a one-shot's single owner = at startup). For my owner(s): list `--filter
   label=a2a.owner=<owner>`, `classify` each (D3), reap **only Dead**. Owner-scoped + same-host; never touches
   Alive/Unknown. Global cleanup of *other* owners' dead orphans is the operator's `containers reap
   --all-dead` (D6), not automatic.
3. **Specific-container reap** (existing per-turn `ContainerReaper` + `retire`/`retire_warm`): unchanged —
   reaps a specific known name (now carrying `run_id`).

**Legacy orphans:** containers from before this change have no `a2a.*` labels. They are **never auto-reaped**
(can't classify safely → could be an old live peer); `containers list` surfaces them (old-pattern names,
list-only) for manual `containers reap --force <name>`.

Pure identity/classify live in **new** `bridge-core/src/run_identity.rs` (`RunHandle`, `Verdict`, `classify`)
+ `bridge-core/src/liveness.rs` (the `flock`/host probe behind a trait); `bridge-core::reaper` grows the
label-scoped sweep + reaps. `:rw` and `:ro` share one mechanism.

### D5 — Stuck-but-alive net: reported + manual staleness (DB-free)

`flock` liveness only catches **dead** owners; a wedged-but-alive run is spared. DB-free signal: the
container's **last-output age** via `docker logs --tail 1 --timestamps <c>`:
- `containers list` shows last-output age + flags `stale` past a threshold (default 1h — no legit silence
  runs an hour; at 1h the warm cache is gone anyway).
- `containers reap --stale [--older-than <dur>]` reaps stale (Alive) containers — operator-initiated.

Staleness reaps **alive** containers, so in A it is **reported + manual only — never automatic**. Automatic +
precise (ACP-stream `last_activity`, so a mid-tool agent isn't misjudged) + `--resume` is Increment B.

### D6 — `containers` command (visibility + manual reap)

```
a2a-bridge containers list                  # run, role, kind, agent, host, Alive/Dead/Unknown, repo, cwd, age, last-output (stale?), name; + legacy unlabeled (list-only)
a2a-bridge containers reap                  # my-owner(s) Dead-only (the boot sweep, on demand)
a2a-bridge containers reap --all-dead       # every owner, this host, Dead-only
a2a-bridge containers reap --run <id> | --owner <hash>   # scoped, Dead-only by default
a2a-bridge containers reap --stale [--older-than 1h]     # reap stale (Alive) — operator override
a2a-bridge containers reap --force <name>   # the ONLY way to reap a specific Alive/Unknown/legacy container
```
Reads purely from docker labels + `docker logs -t` + the `flock` probe. `--run`/`--owner` are **Dead-only**
unless combined with `--force`; `--stale` and `--force` are the *only* ways to kill a live container.
(Namespaced `containers <verb>` leaves room for a future `containers gc` for orphaned clone dirs, R5.)

### D7 — Scope: both `:rw`/`:ro`; one-shots fully isolated; serve protected as a unit

`:rw` (impl) + `:ro` (review readers) both get D1 labels + D2 names + D4 sweeps. **One-shot** processes
(`implement`/`run-workflow`) get full isolation: one process = one run = one lease, so concurrent same-config
one-shots never clash or cross-reap (the footgun). For **`serve`**: the serve process holds one lease;
**process-alive ⇒ all its containers are Alive/spared**. Reaping one request among live siblings needs
per-request leases → **Increment B**. Single host is the operating assumption; `a2a.host` makes a multi-host
shared daemon fail safe (other host → Unknown → spare), not correct.

---

## Architecture / components

| File | Responsibility |
|---|---|
| `crates/bridge-core/src/run_identity.rs` **(new)** | PURE: `RunHandle { instance_id, host, lease_path, .. }`; `Verdict {Alive,Dead,Unknown}`; `classify(labels, host_id, lease_probe) -> Verdict` (D3) — unit-tested with an injected lease/host probe. |
| `crates/bridge-core/src/liveness.rs` **(new)** | The host adapter: stable host id; `flock`-acquire probe over a lease path (non-blocking); lease create/hold/remove. Fail-safe `Unknown` on any error. |
| `crates/bridge-core/src/sandbox.rs` | PURE `a2a_name(role, owner, run_id, seq/nonce)` + `a2a_labels(handle, owner, role, kind, agent, …) -> Vec<(k,v)>`; splice `--name`/`--label` into `compose_container_rw`/`compose_*` (BOTH roles); `by-owner`/`by-run` filter argv + `managed-list` format. |
| `crates/bridge-core/src/reaper.rs` | `classify`-driven label-scoped **sweep** (list-by-label → classify → reap Dead); `run_scoped_reap(run_id)`; `last_output_age` (`docker logs --tail 1 -t`). Preserve the outer `SweepFn` shape; change impl + arg meaning (now owner + classifier). |
| `crates/bridge-container/src/lib.rs` | `:rw` name gains `run_id`; accept run-identity at construction → labels; **drop** the construction owner-wide sweep + the now-dead `sweep_fn` param (recovery moves to the before-first-use sweep in main); `retire`/END stay specific-name. |
| `crates/bridge-acp/src/acp_backend.rs` (`:ro` spawn) | carry `run_id` in the name + the same labels. |
| `bin/a2a-bridge/src/main.rs` | build ONE `RunHandle` + acquire the lease per process; thread it into both spawn paths + serve's closure; per-owner **before-first-use** Dead-only sweep; `Ro/RwSweepGuard` → `a2a.run` unconditional + lease removal; the `containers list|reap` subcommand + dispatch + `TOP_USAGE`. |

## Data flow

1. Startup: build `RunHandle`; create + `flock` the lease.
2. Before first use of an owner: owner-scoped Dead-only sweep (classify via lease; reap dead orphans + remove
   their leases; live peers/other-hosts spared).
3. Spawn: every managed `docker run` carries D1 labels + the `run_id` name.
4. Run: existing specific-container reapers unchanged.
5. Exit: `*SweepGuard` reaps `label=a2a.run=<mine>` then removes my lease.
6. Crash: OS releases my lease lock (file persists) → a later same-owner/same-host run's sweep (or
   `containers reap`) acquires it → reaps my orphans + removes the lease.
7. Stuck-but-alive: surfaced in `containers list` (stale); operator `containers reap --stale`.

## Error handling

- Reaps best-effort (`docker rm -f` of a gone container is harmless).
- `classify` is **fail-safe toward sparing**: other host, probe error, or lease-open error → `Unknown ⇒
  Alive`. A missed orphan is a recoverable lingering container (visible in `list`); a wrong-kill is lost work.
- The before-first-use sweep must not block the run on docker/lease hiccups (best-effort, logged).
- `owner` is a `DefaultHasher` digest (self-consistent within one binary). With flock-based liveness, the
  owner is only a *grouping/recovery* hint, not the safety mechanism — an upgrade that changes the digest at
  worst makes old orphans group differently (still `list`-able + `--force`-reapable), never a wrong-kill.

## Build order (the atomic slice is flagged)

- **S1 — inert identity.** `run_identity` + lease handle scaffolding; `a2a_name`/`a2a_labels`; splice
  `--label` (NOT the `:rw` name yet) for both roles. No behavior change beyond new labels (observable —
  reframed as a compatibility step; old name-based paths still valid until S3). Unit-tested.
- **S2 — flock liveness module.** `liveness` + `classify` behind injected lease/host fixtures. No wiring.
- **S3 — atomic flip (HARD ORDERING / R3).** `run_id` into the `:rw` name → flip boot recovery to
  owner-scoped before-first-use Dead-only (classify via lease) → re-scope `*SweepGuard` to `a2a.run` +
  lease removal → drop the dead `sweep_fn`. One commit; fixes clash + cross-reap together.
- **S4 — `containers list`** (read-only classify + stale flag + legacy surfacing). Zero risk.
- **S5 — `containers reap`** (`--all-dead`/`--run`/`--owner` Dead-only, `--stale`, `--force`).

## Testing

- **Pure unit:** `a2a_labels`/`a2a_name` argv; `classify` over labels × an injected lease/host probe
  (other-host→Unknown; lease-held→Alive; lease-free→Dead; lease-absent→Unknown; probe-error→Unknown); filter
  argv; `last_output_age` parsing.
- **Live gate (Docker):** (a) two concurrent `implement` runs, **same repo + config**, both complete — no
  `--name` clash, no cross-reap; (b) `kill -9` a run mid-flight → its lease lock frees → the next same-owner
  run's before-first-use sweep reaps the orphan while its own containers survive; (c) `containers list` shows
  both with correct Alive/Dead/stale + any legacy unlabeled; `containers reap` reaps only Dead.

## Risks (from clean-room + dual review)

- **R1 multi-host / shared daemon** → `a2a.host` mismatch ⇒ Unknown ⇒ spare (never wrong-kill); single-host
  is the assumption.
- **R2 never auto-reap stuck** → holds by construction (staleness feeds only `list`/`reap --stale|--force`).
- **R3 atomic-flip regression** → the D2 constraint; live-gate (a) is its acceptance test.
- **R4 legacy orphans** → list-only + manual `--force`; never auto-reaped (can't classify).
- **R5 orphan clone dirs** (`.a2a-implement/impl-*`) → filesystem leak, orthogonal; surface read-only in
  `list`, defer GC (future `containers gc`). OUT of scope.

## Forward-compatibility with Increment B

A's labels + lease are B's substrate: B adds a SQLite registry (durable history, restart/`--resume`
lineage), **per-request** leases (the deferred serve isolation), precise ACP-`last_activity` auto-reap +
wedged-owner detection, non-container coverage, and same-target locks. No rework.

## Review provenance

Clean-room `design` cross-check (independent, converged on the spine + caught owner-as-group-key, the atomic
ordering constraint, module boundaries) + dual spec-review (containerized codex+claude→synth PRIMARY +
a2a-local codex BACKSTOP). Decisions folded: **flock-primary liveness** (both flagged pid+start+boot unsafe
on the macOS host — `kern.boottime` drift wrong-kill, weak `lstart`); **serve-request isolation → B**; host
label; legacy-orphan list-only; before-first-use sweep; S1/S3 reframe; `run_id`→`instance_id` rename;
reap Dead-by-default. (Backstop caveat: it ran with its own repo as cwd, so its repo-grounded "no Cargo.toml"
finding was spurious; its design-level findings were used.)
