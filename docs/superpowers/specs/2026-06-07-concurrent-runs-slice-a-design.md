# Concurrency-safe containerized runs — Increment A (design)

**Status:** Proposed (2026-06-07). Brainstormed + scoped with the owner; pending clean-room cross-check +
dual review before planning.

**Goal:** Let multiple containerized `a2a-bridge` runs (`implement`, `run-workflow`, `serve` requests) run
**concurrently against the same OR different repos using one shared config** without clashing or
cross-reaping each other's containers, recover containers orphaned by a **crashed** run, and give the
operator **visibility** into what's running — all with **no database** (docker labels are the registry).

**Non-goals (→ Increment B):** SQLite run registry, precise ACP-stream `last_activity`, *automatic*
staleness reaping, `--resume`-vs-fresh restart lineage, cross-process coordination / same-target locks,
non-container (`api`/ollama) agent coverage.

---

## Problem

A containerized run names + reaps its agent containers by an **owner** = `hash(config_path, mount,
agent_id)` (`container_owner`). Two runs sharing a config therefore share an owner, which breaks concurrency
two ways:

1. **Name clash (`:rw`).** The `:rw` (ContainerRw / `implement`) container is named `a2a-rw-{owner}-{turn_seq}`
   with `turn_seq` from 0, so two same-owner runs both grab `a2a-rw-{owner}-0` → the second `docker run
   --name` fails. (`:ro` reader containers already dodge this with a per-container nonce: `a2a-ro-{owner}-
   {nonce}`.)
2. **Owner-wide boot-sweep cross-reap (`:rw` AND `:ro`).** At startup each backend sweeps `a2a-{rw,ro}-
   {owner}-*` to clean orphans from a *previous crashed* run — but with a shared owner this **kills a
   concurrent live peer's containers**. An `implement` run uses a `:rw` impl agent *and* `:ro` review
   readers, so both must be fixed or a second run still reaps the first's review readers.

Today's only workaround is a distinct config file per concurrent project (distinct `config_path` → distinct
owner) — a footgun the owner has hit.

Separately, the boot-sweep is the *only* automatic crash-orphan recovery, and it can't tell a crashed
orphan from a live peer by owner alone.

---

## Decisions

### D1 — Two-axis identity (`owner` group key + `run_id`), carried on docker labels (docker IS the registry)

**`owner` stays the stable GROUP key** = `hash(config_path, mount, agent_id)` (unchanged). It lets a
*restarted* bridge find its OWN crash-orphans by label (an ephemeral-only id couldn't). **Add a per-process
`run_id`** (a UUID minted at startup) as the run identity: same-config concurrent runs share an `owner` but
never a `run_id`.

Each bridge **process** (one `implement`/`run-workflow`, or one `serve`) mints `run_id`, and captures its
**PID + a process start-token + a boot token** (D3). Every managed container — `:rw` and `:ro` — is launched
with:

```
--label a2a.managed=1
--label a2a.role={rw|ro}              --label a2a.kind={warm|perturn|oneshot}   --label a2a.agent=<agent id>
--label a2a.owner=<owner hash>       --label a2a.run=<run_id>
--label a2a.pid=<owning pid>         --label a2a.pid_start=<start-token>        --label a2a.boot=<boot token>
--label a2a.config_hash=<hash>       --label a2a.repo=<repo>   --label a2a.cwd=<cwd>   --label a2a.start=<rfc3339>
```

Label VALUES must be docker-safe — identity keys are hashes (`owner`, `config_hash`, `pid_start`); `repo`/
`cwd` are display-only + best-effort (sanitized; absent if N/A). No DB:
`docker ps -a --filter label=a2a.owner=<owner> --format '{{.Label "a2a.run"}}\t{{.Label "a2a.pid"}}\t…'` is
the query surface. These labels are exactly the columns Increment B's rows will key on — A is B's substrate.

### D2 — `run_id` in the name (kills the clash outright)

`:rw` joins `:ro` in carrying a per-run token in the name:
```
a2a-rw-<owner>-<run_id>-<seq>        a2a-ro-<owner>-<run_id>-<nonce>
```
Two runs share `owner` but differ in `run_id`, so they can never collide on a `--name`. Grouping/reaping is
by **label** (`a2a.owner`/`a2a.run`), not name substring. The existing per-turn/`retire` reapers keep reaping
a specific known name (now carrying `run_id`) — unchanged; only the owner-wide *sweeps* change (D4).

> **HARD ORDERING CONSTRAINT (highest-risk correctness point).** The `:rw` `run_id`-in-name change (D2) and
> the boot-sweep flip (D4.2) MUST ship in the **same slice, name-first**. Today a second run's boot-sweep
> reaps the first's `a2a-rw-{owner}-0`, *freeing the name* for its own mint; if D4.2 stops reaping live peers
> before D2 gives each run a unique name, the second `docker run --name` hard-fails on the duplicate. The
> clash and the cross-reap are one event — fix them as one atomic change.

### D3 — Dead-owner liveness = PID + start-token + boot-token (`Verdict {Alive,Dead,Unknown}`)

`classify` a container's owner from its labels:
- **boot-token** differs from the host's current boot id → the machine rebooted (all PIDs reused) → **Dead**.
- else `kill -0 <a2a.pid>` fails → **Dead**.
- else the live PID's start-token ≠ labeled `a2a.pid_start` → PID reuse → **Dead**.
- start-token matches → **Alive**.
- any descriptor unreadable / probe error → **Unknown ⇒ treated as Alive (fail-safe)**.

So a dead owner is detected *deterministically* (PID reuse and reboot can no longer hide it), and liveness
**never wrong-kills a live peer** — the cost of ambiguity is a recoverable lingering orphan, never lost work.

Portable probe (one shared helper used for BOTH labeling + checking, so they compare equal on the same host):
start-token = Linux `/proc/<pid>/stat` field 22, **macOS `ps -o lstart= -p <pid>`** (our dev host is macOS);
boot-token = Linux `/proc/sys/kernel/random/boot_id`, macOS `sysctl -n kern.boottime`. All normalized to hex.
(Documented fallback if process introspection proves brittle: a `flock` lease file per `run_id` — the OS
releases it on crash, fully cross-platform — at the cost of a lockfile lifecycle + dep. Not chosen for A.)

### D4 — Reap scopes, all label-scoped (replacing the owner-wide sweep)

1. **This run's cleanup** (END-sweep `RwSweepGuard`/`RoSweepGuard`): reap `--filter label=a2a.run=<my run_id>`
   **unconditionally** → only *my* containers (these deliberately clean my own live containers on exit;
   gating them would reintroduce the leak they prevent). A peer's `a2a.run` differs → never cross-reaps.
2. **Crash-orphan recovery — boot sweep** (at process startup): list `--filter label=a2a.owner=<my owner>`,
   `classify` each, reap **only Dead**. Owner-scoped (each config recovers its OWN crash-orphans across a
   restart); never touches Alive/Unknown. Self-exclusion is free — at boot we've minted nothing, and any
   concurrent peer classifies Alive. (Global cleanup of *other* owners' dead orphans is the operator's
   `containers reap --all-dead`, D6 — not automatic.)
3. **Specific-container reap** (the existing per-turn `ContainerReaper` + `retire`/`retire_warm`): unchanged —
   they reap a specific known name (now carrying `run_id`).

Pure `classify`/identity live in **new** focused modules `bridge-core/src/run_identity.rs` (`RunHandle`,
`Verdict`, `classify`) + `bridge-core/src/liveness.rs` (the host probe); `bridge-core::reaper` grows the
label-scoped sweep + reaps. `:rw` and `:ro` share one mechanism.

### D5 — Stuck-but-alive net: reported + manual staleness (DB-free)

Liveness (D3) only catches **dead** owners; a wedged-but-alive run is spared. A DB-free staleness signal —
the container's **last-output age** via `docker logs --tail 1 --timestamps <c>` — catches it:

- `a2a-bridge containers list` shows each managed container's last-output age and flags `stale` when older
  than a threshold (default 1h — no legitimate silence runs an hour, and at 1h the warm cache is gone anyway).
- `a2a-bridge containers reap --stale [--older-than <dur>]` reaps stale containers on the operator's say-so.

Staleness reaps **alive** containers (the wrong-kill-able kind), so in A it is **reported + manual only** —
**never automatic**. Automatic + precise staleness (driven by the ACP-stream `last_activity`, so a mid-tool
agent isn't misjudged) + `--resume` is Increment B. (A bridge-written heartbeat label/file is the clean-room's
alternative staleness signal; deferred — `docker logs` age needs nothing from the run.)

### D6 — `containers` command (visibility + manual reap)

```
a2a-bridge containers list                      # table: run, role, kind, agent, pid Alive/Dead/Unknown, repo, cwd, age, last-output (stale?), name
a2a-bridge containers reap [--all-dead]         # liveness sweep now (Dead only); --all-dead spans every owner, not just mine
a2a-bridge containers reap --run <id> | --owner <hash>   # scoped reap
a2a-bridge containers reap --stale [--older-than 1h]     # reap stale (Alive) containers — operator-initiated
a2a-bridge containers reap --force <name>       # the sole Alive/STUCK override
```

Reads purely from docker labels + `docker logs -t`. The operator's answer to "what's running / clean up the
dead/stuck ones" without a shell. (Namespaced `containers <verb>` over a flat `ps`/`reap` so it can grow,
e.g. a future `containers gc` for orphaned clone dirs — see R5. Naming is the owner's call; low-stakes.)

### D7 — Scope: both `:rw` and `:ro`

`implement` uses a `:rw` impl agent and `:ro` review readers; `run-workflow`/review use `:ro`. Both get the
labels (D1) + `run_id`-in-name (D2), the run-scoped END-sweep (D4.1), and are covered by the boot sweep
(D4.2) and `containers` (D6). The shared `bridge-core` mechanism makes this one implementation, not two.

---

## Architecture / components

| File | Responsibility |
|---|---|
| `crates/bridge-core/src/run_identity.rs` **(new)** | PURE: `RunHandle { owner, run_id, pid, pid_start, boot }`; `Verdict {Alive,Dead,Unknown}`; `classify(labels, host) -> Verdict` (D3). Unit-tested with injected host fixtures. |
| `crates/bridge-core/src/liveness.rs` **(new)** | The host probe behind a trait: `kill -0`, start-token (`/proc` or `ps`), boot-token (`/proc` or `sysctl`). Fail-safe `Unknown` on any read error. |
| `crates/bridge-core/src/sandbox.rs` | PURE: `a2a_name(role, owner, run_id, seq/nonce)`; `a2a_labels(handle, role, kind, agent, …) -> Vec<(k,v)>`; splice `--name`/`--label` into `compose_container_rw`/`compose_*` (BOTH roles); filter-argv builders (`by-run` / `by-owner` label filters; `managed-list` format). |
| `crates/bridge-core/src/reaper.rs` | PURE `classify`-driven decision over a candidate list; the label-scoped **sweep** (docker-list-by-label → classify → reap Dead); `run_scoped_reap(run_id)`; `last_output_age` (`docker logs --tail 1 -t`). Preserve the outer `SweepFn` shape (single consumer) — change only the impl + arg meaning. |
| `crates/bridge-container/src/lib.rs` | `:rw` name gains `run_id` (mint site); accept run-identity at construction → labels; **drop** the owner-wide boot-sweep here (recovery moves to the process-level boot sweep in main); `retire`/END stay specific-name. |
| `crates/bridge-acp/src/acp_backend.rs` (`:ro` container spawn) | carry `run_id` in the name + the same label-set on the `:ro` `docker run`. |
| `bin/a2a-bridge/src/main.rs` | build ONE `RunHandle` per process (mint `run_id`, capture pid/start/boot); thread it into both spawn paths (`acp_spawn_inputs`/`AcpContainerSpawn` + the ContainerRw config) + serve's spawn closure; `RoSweepGuard`/`RwSweepGuard` → `a2a.run=<run_id>` unconditional; an **owner-scoped boot sweep** (Dead-only) before work; the **`containers list|reap`** subcommand + dispatch + `TOP_USAGE`/unknown-subcommand update. |

## Data flow

1. Startup: build the `RunHandle` (mint `run_id`; capture pid/start/boot); run the **owner-scoped boot sweep**
   (`classify` `a2a.owner=<mine>` candidates; reap Dead only; live peers + self classify Alive).
2. Spawn: every managed `docker run` carries the D1 labels + the `run_id` name.
3. Run: existing specific-container reapers handle per-turn/`retire` as today.
4. Exit (any path): the run-scoped `*SweepGuard` reaps `label=a2a.run=<my run_id>` unconditionally (only mine).
5. Crash (no exit): the orphan's owner PID/boot mismatches → a *later* same-owner process's boot sweep (or
   `containers reap [--all-dead]`) reaps it; PID reuse + reboot can't hide it (D3).
6. Stuck-but-alive: surfaced in `containers list` (stale flag); operator reaps via `containers reap --stale`.

## Error handling

- All reaps are best-effort (`docker rm -f` of a gone container is a harmless ignored error).
- `classify` is **fail-safe toward sparing**: any ambiguity (unreadable descriptor, docker/probe error) →
  `Unknown ⇒ Alive` → do NOT reap. A missed orphan is a recoverable lingering container (visible in
  `containers list`); a wrong-kill is lost work — bias hard against wrong-kill.
- The boot sweep must not block the run on docker hiccups (best-effort, logged).
- `owner` is a `DefaultHasher` digest — deterministic within one binary build (so "stable across restarts"
  holds), not across std versions. One binary owns both label-write + sweep, so it's self-consistent; note,
  don't engineer around it.

## Build order (smallest shippable slices; the atomic one is flagged)

- **S1 — inert identity.** `run_identity` + `a2a_name`/`a2a_labels`; splice `--name`/`--label` into `compose_*`
  for BOTH roles. No behavior change. Unit-tested.
- **S2 — liveness module.** `classify` + the host probe behind injected fixtures. No wiring.
- **S3 — atomic behavioral flip (HARD ORDERING / R3).** `run_id` into the `:rw` name → flip BOTH boot sweeps
  to owner-scoped Dead-only → re-scope `*SweepGuard` to `a2a.run`. **This one slice fixes the clash + the
  cross-reap together** (see D2 constraint); ship it as one commit.
- **S4 — `containers list`** (read-only inspect + classify + stale flag). Zero risk; immediate visibility.
- **S5 — `containers reap`** (`--all-dead` default Dead-only / `--run` / `--owner` / `--stale` / `--force`).

## Testing

- **Pure unit:** `a2a_labels`/`a2a_name` argv; `classify` over synthetic labels × an injected host
  (`Dead`: pid-free / reused-pid mismatch / boot mismatch; `Alive`: full match; `Unknown→Alive`: unreadable);
  the `by-owner`/`by-run` filter argv; `last_output_age` parsing.
- **Live gate (Docker):** (a) two concurrent `implement` runs, **same repo + same config**, both complete —
  no `--name` clash, no cross-reap (each run's containers survive while the other runs); (b) start a run,
  `kill -9` it mid-flight, then start another same-owner run → the orphan is reaped by the new run's boot
  sweep while the new run's own containers survive; (c) `containers list` shows both with correct
  Alive/Dead/stale; `containers reap` reaps only the Dead.

## Risks (from the clean-room cross-check)

- **R1 non-Linux host** → liveness fail-safes `Unknown→Alive` (recoverable leak, never a wrong-kill).
- **R2 never auto-reap stuck** → holds by construction (staleness feeds only `list`/`reap --stale|--force`).
- **R3 atomic-flip regression** → the D2 ordering constraint; live-gate (a) is its acceptance test.
- **R5 orphan clone dirs** (`.a2a-implement/impl-*`) → a filesystem leak orthogonal to containers; surface
  read-only in `containers list`, defer GC (a future `containers gc`) — OUT of scope.

## Forward-compatibility with Increment B

A's labels (`a2a.run/pid/pid_start/repo/cwd/start`) are B's row keys; A's `runs` becomes DB-backed + richer;
A's coarse reported staleness becomes B's precise ACP-`last_activity` auto-reap; B adds wedged-*owner*
detection, durable history, restart/`--resume` lineage, non-container coverage, and same-target locks. No
rework.
