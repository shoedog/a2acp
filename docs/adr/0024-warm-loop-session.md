# ADR-0024 — Warm Loop Session (Containerized Agents, Slice B2b-3c)

**Date:** 2026-06-07
**Status:** Accepted

**Builds on:** B2a (`ContainerRwBackend`, ADR-0018), B2b-1 (`implement`, ADR-0019), B2b-2 (verify,
ADR-0020), the `:ro` reaper (ADR-0021), B2b-3a (review-the-diff, ADR-0022), B2b-3b (review→tweak loop,
ADR-0023).

---

## Context

B2b-3b closes the self-correcting loop (`verify → review → classify → fix → amend`), but every impl turn —
the first edit AND each fix — ran on a **cold per-turn** `ContainerRwBackend`: a fresh container + a fresh
ACP session per turn. So the fix agent was a stranger to its own prior edit (no conversational continuity),
and each turn paid a container + handshake cold-start. The impl turns were structurally forced apart: the
executor mints `SessionId = workflow-{wf}-{node}-{run}`, so the edit session and each fix session could
never coincide.

## Decision

Warm the impl agent: **ONE container + ONE ACP session reused across all the loop's edit + fix turns**,
reaped only at the end. Review/verify are unchanged.

- **In-place `Lifecycle::{PerTurn,Warm}` on `ContainerRwBackend`, with SEPARATE warm bodies.** A one-line
  `if self.is_warm() { return self.<op>_warm(...).await }` guard at the top of `prompt`/`cancel`/`retire`
  delegates to `prompt_warm`/`cancel_warm`/`retire_warm`. The never-reap invariant lives in isolated warm
  code a future per-turn edit cannot reach (the spec's "single injected reap-trigger", realized concretely)
  — NOT scattered per-line branching. The per-turn path is behaviorally unchanged.
- **Extracted `open_inner`** (spawn + compose + configure + spawn-failure reap) shared by the per-turn
  `prompt` and the warm cache-miss path — one source of truth for naming/compose/configure.
- **Authoritative `warm` cache + `turn_active` marker + `TurnGuard`.** `prompt_warm` opens once (cache miss)
  or reuses (re-applies the cached canonical cwd); `turn_active` is the concurrency reject, cleared by a
  `TurnGuard` on stream end (synchronously) or early drop (detached). The `TurnGuard` **NEVER reaps**;
  `retire_warm` (which also clears `turn_active`) is the **sole** warm reap site.
- **Reuse-turn errors never reap.** A `configure`/`prompt` error on a *reuse* turn clears `turn_active`,
  does NOT reap, and returns `Err` — a transient error must not nuke the warm container (the loop converts
  it to `FixIncomplete` via `TweakEffects::fix -> bool`). A *cache-miss* prompt error reaps + removes the
  just-opened entry (no cumulative work to protect).
- **Impl turns OFF the executor.** `implement_cmd` resolves the impl agent with the pure
  `resolve_impl_identity` (edit & fix workflows must each be single-node and name the SAME ContainerRw
  agent — fail-loud pre-first-commit), builds a warm backend from that entry, mints ONE stable
  `SessionId("implement-{task_id}")`, and drives the edit turn + `ProdEffects::fix` as `warm.prompt(...)`
  calls drained by `drain_turn`. **Review stays on the executor/registry** (built afterward; edit/fix don't
  use it).
- **`drain_turn` is stricter than the executor.** Complete IFF a `Done{stop_reason != cancelled}` arrived;
  a stream `Err(_)` or a clean end without `Done` → incomplete (the executor leaves `ok=true` on a clean
  end — a false positive `drain_turn` deliberately avoids).
- **`RwSweepGuard` END-sweep backstop**, owner-aligned with the warm backend's spawn-time owner via the
  shared `container_owner(config_path, mount, agent_id)`; declared BEFORE `warm` so it drops AFTER it.
  `retire()` runs on EVERY terminal arm (Abort/NoCommitClean/NoCommitDirty/Commit); the Commit arm prints
  the hand-off BEFORE `retire` so a retire error never suppresses it (log-only, never alters the result).

## Why in-place (not a separate type)

A separate warm type would re-implement the whole `AgentBackend` surface + plumbing; reuse of `open_inner`
is the real argument for in-place. The cost — a dual state machine on one struct — is contained to the
isolated `*_warm` bodies behind the one-line dispatch, so the per-turn safety contract (reap-on-exit) and
the warm contract (never-reap-except-retire) can't be cross-contaminated by an edit to either.

## Validation

- **Idle-survival spike PASSED** (the gating empirical risk): a warm ACP session answered a second prompt
  after a 420 s idle (proxy for the verify+review gap) — `stopReason=end_turn`, container still up.
- **Live gate PASSED** (Docker), run with **codex** as the `:rw` impl agent:
  - *Right-first-try:* warm opens, ONE container, edit turn → commit (bot identity) → converge (1 attempt)
    → reaper → 0.
  - *Converge-via-fix:* edit → verify-FAIL → multiple fix turns → verify PASS → converge (3 attempts).
    The **same-container-id-across-turns** assertion is proven: a container watcher shows ONE
    `a2a-rw-<owner>-0` (nonce `-0`) **Up continuously across all loop attempts** — no second container ever
    spawned (a cold per-turn path would mint `-1`, `-2`, …). Reaper → 0.

## Findings / notes

- **codex works as a `:rw` impl agent** with `-c sandbox_mode="danger-full-access"`. The recorded
  in-container codex repo-blindness was codex's OWN sandbox (bubblewrap) failing to init — `bwrap` is
  absent from the toolchain image. Disabling codex's internal sandbox is correct here: the Docker container
  IS the sandbox. (claude remains the default impl; codex avoids the claude OAuth-expiry friction.)
- **Verify-robustness aside (follow-up, not a B2b-3c issue):** `cargo fmt --all -- --check` does not visit
  a freshly-added module file the way `rustfmt` direct does, and an agent can churn untracked `rustfmt.toml`
  `ignore` configs that survive the worktree reset. A future verify-hardening could `git clean` + reset to a
  pristine committed tree before each verify.

## Code review (dual: containerized codex+claude PRIMARY + a2a-local codex BACKSTOP)

The built diff was dual-reviewed before merge. **Folded fixes:**

- **(backstop, HIGH — real regression)** The warm `SessionSpec` used `config: Default::default()`, dropping
  the impl agent's model/mode/**effort** (the executor stashes `effective_config`; `AcpBackend` prefers the
  stashed spec). Fixed to `effective_config(&impl_entry, None)`.
- **(both)** `TurnGuard`'s early-drop clear could erase a *later* turn's `turn_active` marker — `turn_active`
  is now `HashMap<SessionId, u64>` and every clear (sync + detached) is **epoch-guarded**.
- **(both)** `prompt_warm` re-fetched the warm entry with `.unwrap()` across awaits → a concurrent `retire`
  could panic. Now returns a handled `Err` ("retired during prompt").
- **(backstop)** Post-edit `?` paths (`stage_state`, `host_commit`) now `retire()` before returning (a
  container exists by then; `RwSweepGuard` stays the backstop).
- **(primary)** `drain_turn` now **latches** completion (a trailing teardown `Err` after a successful `Done`
  no longer un-completes the turn).

**Known constraints / deferred (documented, not reachable in current sequential usage):**

- **Two execution engines (HARD CONSTRAINT).** Edit/fix run via `warm.prompt` + `drain_turn`; review runs on
  the executor. `resolve_impl_identity` rejects anything but a single-node graph, so the warm edit/fix path
  cannot gain multi-node graphs, per-node cancel/timeout, or `NodeStarted/Finished/Terminal` telemetry. A
  future workflow-model feature will NOT apply to edit/fix unless the executor learns to run on a
  caller-supplied warm session.
- **Implicit "always fully drain, never cancel mid-stream" contract.** The warm concurrency hazards are
  unreachable today only because `run_tweak_loop`/`drain_turn` always poll to stream end and nothing cancels
  mid-stream. Deferred hardening for when the loop gains concurrency/early-drop: a `Reserving/Live/Retiring`
  warm state machine; making `Lifecycle` an enum that OWNS its state (`PerTurn(Inflight)` /
  `Warm{warm,turn_active}`) so a method can't reach the wrong mode's fields; and distinguishing a fatal
  child loss (reap + respawn next turn) from a recoverable reuse-turn error.

## Consequences

- Continuity (one session) + no per-turn cold start within a loop; the container is reaped at `retire`
  with the `RwSweepGuard` as a synchronous backstop.
- Per-turn (`PerTurn`, the default) behavior is unchanged — all B2a/B2b tests stay green.
- A warm-**pool** (multiple concurrent warm sessions per backend) remains future work; this slice warms one
  session per `implement` run.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
