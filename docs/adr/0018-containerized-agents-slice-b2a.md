# ADR-0018 — Per-Turn `:rw` ContainerRwBackend (Containerized Agents, Slice B2a)

**Date:** 2026-06-05
**Status:** Accepted

**Builds on:** ADR-0017 (Slice B1 — the enforced `[sandbox]` block for `:ro`/Acp readers). B1 composed +
enforced containment on the **warm** path and **rejected** `access=rw` ("requires the container_rw kind").
B2a unlocks `:rw`. First sub-slice of Slice B2 (B2b = the `implement` workflow + per-task git clone + verify
+ human-approval, follows).

---

## Context

B1 shipped read-only containerized agents. A write-capable agent (the foundation for an `implement` step
that edits + commits) needs a **writable** mount and a **fresh, isolated container per task** — not the
warm, multiplexed-over-one-process model the `:ro` readers use (a writer's `:rw` target differs per task,
so it can't multiplex). The dual spec-reviews also surfaced that the obvious "warm-per-session writer" is
materially more concurrency surface than B2a needs, and that the run-context-owned clone (B2b) — not a warm
container — carries work continuity. So B2a ships the **per-turn** foundation; a warm-pool for writers is a
separate future slice (`docs/superpowers/specs/2026-06-05-containerized-agents-warm-pool-slice.md`).

## Decision

A new `AgentKind::ContainerRw` whose `ContainerRwBackend` (new crate `crates/bridge-container`) spawns a
**fresh `:rw` container per `prompt` turn** by composing `bridge-acp::AcpBackend`, and reliably reaps it.

- **Pure composers** (`bridge-core/sandbox.rs`, beside `compose_sandbox`): `compose_container_rw` (reuses
  `compose_sandbox` with `mount=rw_target` + `access=Rw`, splices a unique `--name` after `--rm`),
  `reap_argv`, and `check_rw_target` (pure lexical containment on **already-canonicalized** inputs — the
  module stays I/O-free).
- **`ContainerRwBackend`** (composes `AcpBackend` via a `ContainerSpawn` injection seam so the lifecycle is
  Docker-free testable):
  - **Per-turn**, stream-owned reaper: the returned stream owns a `ContainerReaper` that reaps on every
    terminal path (Done / error / consumer-drop / cancel); reap is **idempotent** + **detached** (`Drop`
    never blocks a Tokio worker).
  - **Strict-reject** when no session cwd — a writer must name its `:rw` target (no fallback to the broad
    root).
  - **Canonicalizing rw-target guard** (the I/O lives here, not in the pure module): canonicalizes BOTH the
    mount anchor and the target (resolving symlinks — a symlink can't escape the root), nearest-existing-
    ancestor for a not-yet-existing scratch dir.
  - **Atomic check-and-reserve** (`InflightState::{Reserving, Live}`) — a concurrent second prompt on a live
    session is rejected without a check-then-insert race; the `Live` handle lets `cancel` reach the inner;
    cancel + stream-drop share one `reaped` flag (no double-reap).
  - Canonical cwd forwarded to the inner (AcpBackend prefers the stashed cwd over `AcpConfig.cwd`);
    spawn/configure/prompt-failure all reap before returning.
  - **Stable-identity boot-sweep**, blocking-at-construction: `owner = hash(config-path + mount + agent_id)`
    so a restarted process reaps its OWN crash orphans (and `turn_seq` restarting at 0 can't collide with a
    surviving orphan); runtime-parametric (`docker`|`podman`).
- **Validation** (`bridge-registry`): a `ContainerRw` arm requires `cmd`+`[sandbox]`, forbids `base_url`,
  PERMITS `access=rw`; S3/S5/S6 are shared with the `Acp` arm via an extracted `validate_sandbox`; `Acp`
  keeps its S4 reject; `Api` keeps its sandbox reject. The reuse predicate already keys on
  `kind`+`sandbox`+`session_cwd`, so a `ContainerRw` entry self-isolates.
- **Wiring**: a `ContainerRw` arm in BOTH `SpawnFn` closures (identical text) + a production
  `AcpContainerSpawn { policy }` (applies the system policy to the inner, mirroring the `Acp` arm). The
  containment anchor is `entry.sandbox.mount` (== normalized `allowed_cwd_root`, S2) because
  `allowed_cwd_root` is NOT on `RegistrySnapshot` and is out of scope in both closures.
- **`run-workflow --session-cwd <dir>`**: threads `WorkflowRunContext.session_cwd` so workflow agents work
  in the target dir, not the launch cwd (closes a dogfood gap where `design` ran in the wrong repo and the
  codex lens bailed) and restores the acceptance gate's run-workflow path.

## Consequences

- **Per-turn memory asymmetry**: a `container_rw` agent gets a fresh container + ACP session each turn, so
  it does NOT retain conversational memory across turns in interactive `serve` (unlike the warm `:ro`
  reader). Work continuity comes from the shared `:rw` target on the host. Documented in the runbook.
- The clean-room `design`/review prompts now read the real code AND tolerate a wrong/missing cwd (state the
  gap + flag assumptions, never bail) — NOT brief-only (reading the code is the value).
- B2a writes only to a scratch dir (overwrite-idempotent), so a detached-workflow resume is safe; the
  non-idempotent git-write hazard arrives with B2b and is gated there.

## Validation

Pure composers + validate arm + backend lifecycle (12 Docker-free tests via the spawn seam: warm-reuse
counter, spawn-failure reap, atomic reject-second, cancel-reaches-inner + no double-reap, off-runtime drop,
symlink-escape + nearest-ancestor, owner-scoped boot sweep). **Live Docker gate (PASS):** `run-workflow
impl-smoke --session-cwd …/.b2a-scratch` → file persists on host; `docker events` show
`a2a-rw-<owner>-0` `start`→`die`→`destroy` (positive containment + reap); no leftover container. Workspace
coverage 89.66% region / 90.72% line (floor 85); `bridge-core/sandbox.rs` 100%; `bridge-registry` 96.75%;
clippy `-D warnings` clean.

## Alternatives considered

- **Warm-per-session writer** (reap on `forget_session`): rejected — `forget_session` fires per-producer-
  exit (not session-end), so it would kill warmth; warm-done-right needs a warm-pool (its own slice).
- **From-zero ACP reimpl** instead of composing `AcpBackend`: rejected — reuse the hardened ACP machinery.
- **`check_rw_target` canonicalizing inside the pure module**: rejected — canonicalization is I/O; the pure
  module stays I/O-free and the backend does the canonicalization.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
