# Warm Loop Session — Design (B2b-3c)

**Date:** 2026-06-06
**Status:** Draft (rev1 — pre clean-room cross-check + dual spec-review).
**Builds on:** B2b-3b (the review→tweak loop, ADR-0023), B2a (`ContainerRwBackend`, ADR-0018), the `:ro`/`:rw`
reaper primitives (ADR-0021), B1 sandbox (ADR-0017). Foundation for the deferred **serve warm-pool** slice
(`docs/superpowers/specs/2026-06-05-containerized-agents-warm-pool-slice.md`).

## Goal

Give the `a2a-bridge implement` review→tweak loop a **warm agent session across its turns**: the edit turn and
every fix turn run on ONE long-lived `:rw` container + ONE ACP session, so the fix turn **continues the same
conversation** (the agent remembers its edit + why verify/review failed) and reuses the container (no per-turn
cold start). Scoped to ONE `implement` run — opened before the first turn, reaped at loop end. The general
multi-session **serve warm-pool** (TTL/idle-eviction, mint-race) is a SEPARATE follow-on that wraps this.

## Why (post-B2b-3b)

The B2b-3b loop runs edit + up to `max_attempts-1` fix turns on one clone; today each is a separate workflow
run that spawns a FRESH per-turn `ContainerRwBackend` (new `docker run claude-agent-acp` + new ACP session,
reaped). So the fix agent starts COLD each turn with no memory of the edit, and the loop re-primes it with a
full failure digest. Warm continuity should both cut N cold starts and improve fix quality (the agent reasons
from what it actually did, not a re-read digest). The live gate showed exactly this multi-cold-turn shape.

## Decisions

1. **Approach A — loop-owned warm session; impl turns leave the executor; review/verify unchanged.** The
   workflow executor forgets the session after every node (DAG cleanup the review relies on), so cross-turn
   warmth can't ride the executor without changing shared teardown. Instead the loop OWNS the warm session
   directly. Review keeps the executor + its own `:ro` reviewer containers; verify keeps its toolchain
   `docker run`. Only the **impl** agent (edit + fixes) becomes warm. (Approach B — warm `ContainerRwBackend`
   + a stable session id + an executor "keep-warm" flag — was rejected: it changes load-bearing executor
   teardown and pulls serve-pool machinery forward.)

2. **`WarmContainerSession` (new, `crates/bridge-container`)** — one `:rw` container + one live `AcpBackend` +
   one ACP session, held across prompts:
   - `open(agent_cfg, clone_cwd)` (the `impl` `AgentEntry` carries the sandbox cfg) → `compose_container_rw`
     argv (REUSED from B2a) → spawn the `AcpBackend`
     (`docker run`) via the existing `ContainerSpawn` seam → `configure_session(clone_cwd)` once. Named
     `a2a-rw-<owner>-<nonce>` (REUSED) for leak-safe reaping.
   - `prompt(text) -> WorkflowStream`/event stream → `backend.prompt(session, text)` on the FIXED session id.
     `AcpBackend` already does `session/new` ONCE (its per-session `OnceCell`) + serializes turns
     (`turn_lock`), so prompt 2..N continue the SAME conversation — continuity is free once the backend stays
     alive across prompts. The `minted_cwd` guard already errors if the cwd changes (it won't — same clone).
   - `cancel()` + `reap()` → drop the backend (kills the child) + `docker rm -f` by name via the shared
     reaper primitives (`bridge_core::reaper`). Idempotent (shared `reaped` flag), reaped on EVERY path.
   `ContainerRwBackend` (per-turn) STAYS for the registry/serve path; both share
   `compose_container_rw`/`check_rw_target`(canonicalized)/the spawn seam/the reaper.

3. **Continuity via the existing `AcpBackend` warm-session machinery** — no new session logic. `open` does
   the one `session/new` (lazily, on the first `prompt`, through `AcpBackend`'s `OnceCell`); subsequent
   prompts are follow-ups in that session. The bridge still host-commits/amends the agent-staged index
   between prompts exactly as B2b-3b (the commit is host-side; the warm session is unaffected by it).

4. **Loop integration — open before the first turn, thread through `ProdEffects`, reap at loop end.** The
   `TweakEffects` seam is UNCHANGED (the fake-executor loop tests stay intact):
   - resolve the `impl` `AgentEntry` from the snapshot (via the edit/fix workflows' `node.agent`) →
     `WarmContainerSession::open(impl_cfg, clone_cwd)` BEFORE the first edit turn.
   - edit turn = `warm.prompt(<implement-edit template rendered with the task>)` → drain → host-commit.
   - `ProdEffects::fix(attempt, input)` = `warm.prompt(<fix follow-up>)` on the SAME session → amend.
   - after `run_tweak_loop` returns → `warm.reap()` (RAII guard so it also fires on early return/panic/cancel).
   The `implement-edit`/`implement-fix` workflows remain the **config source** for "which agent + which prompt
   template" (the loop loads the graphs, reads `node.agent` + `node.prompt_template`); they are no longer
   *executed* per turn — the loop drives their prompts on the warm session.

5. **Fix prompt becomes a follow-up.** Because the agent remembers the task + its edit, the fix turn no longer
   re-primes. `tweak::build_fix_input` → `build_fix_followup` (failures-only: the verify gate-failure digest +
   any REJECT findings + "fix these on the current clone, re-stage, do NOT commit / write a message"). The
   pure budget/format logic is otherwise unchanged. `prompts/implement-fix.md` reframes from "you have a prior
   commit" to "continue — here is what failed" (keeps the firm MUST-`git add` contract from the B2b-3b
   live-gate fix).

6. **Two-phase fallibility preserved.** `WarmContainerSession::open` + the FIRST edit prompt are PRE-first-
   commit → keep `?`/fail-loud (a mint failure leaves no commit). A warm prompt failing MID-loop (agent
   crashed / stream error on a fix turn) is phase-2 → reduce to a new `StopReason::WarmSessionLost` and hand
   off the committed work (NO mid-loop cold re-mint — keep it simple; the committed state survives). The
   always-print hand-off invariant holds.

7. **Reaper — one held container, reaped on all paths.** The warm `:rw` container lives for the whole loop
   (idle-but-alive across the minutes-long verify + review gaps — one extra held container, bounded,
   acceptable; `docker pause` during the gap is deferred). Named → the existing owner-scoped boot-sweep + a
   loop-end sweep guard catch leaks; reaped on success/abort/cancel/Drop via the shared reaper. The `:ro`
   review reaper + the toolchain verify containers are unaffected.

8. **Falsifiable acceptance gate.** A "some `a2a-rw-*` present" check green-falses on a per-turn re-mint. The
   live gate asserts **container IDENTITY**: the SAME container id is alive across the edit turn AND a fix
   turn (one warm container, not two), plus a nonzero check in the inter-turn (verify/review) gap — so a
   silent regression to per-turn fails the gate.

## Component / file boundaries

| Concern | Home |
|---|---|
| `WarmContainerSession` (open / prompt / cancel / reap) — one warm `:rw` container+session; shares compose/check/spawn/reaper with `ContainerRwBackend` | `crates/bridge-container/src/lib.rs` (NEW alongside the per-turn backend) |
| pure `build_fix_followup` (failures-only) + `StopReason::WarmSessionLost` + its `loop_outcome_suffix` arm | `bin/a2a-bridge/src/tweak.rs` |
| resolve `impl` `AgentEntry` + prompt templates; open the warm session; edit turn via `warm.prompt`; `ProdEffects::fix` via `warm.prompt`; loop-end reap guard | `bin/a2a-bridge/src/main.rs` (`implement_cmd` + `ProdEffects`) |
| reframed continuation prompt | `prompts/implement-fix.md` |

(`tweak::run_tweak_loop` + the `TweakEffects` trait signatures are UNCHANGED — `ProdEffects` swaps its `fix`
body from an executor run to `warm.prompt`; the fake-executor tests are untouched.)

## Testing

- **Unit (Docker-free, via the `ContainerSpawn` seam):** `WarmContainerSession` — mint ONCE (first prompt
  spawns one inner; prompts 2..N REUSE the same inner, no re-spawn); `session/new` fired once; `reap` once
  + idempotent (cancel + drop don't double-reap); mint/handshake-failure reaps the named container. Pure
  `build_fix_followup` (failures-only format, budget); `loop_outcome_suffix(WarmSessionLost)`. The B2b-3b
  fake-executor `run_tweak_loop` tests stay green (seam unchanged).
- **Live gate (Docker, dogfooded):** (1) right-first-try → warm opened, ONE prompt, converged, reaped (0
  leaked). (2) **converge-via-fix showing continuity + identity** → an acceptance-orthogonal failure
  (clippy::ptr_arg, per the B2b-3b gotcha) → fix turn continues the SAME session → assert the SAME container
  id across the edit + fix turns (+ nonzero in the gap), one amended commit, converged. (3) reaper holds
  across the run (warm `:rw` → 0 after loop end; `:ro` review + verify containers unaffected).

## Deferred (slice-sized)

- The **serve warm-pool** (the 2026-06-05 stub): wrap `WarmContainerSession` in a `HashMap<SessionId,_>` +
  idle/TTL eviction + exactly-once mint-race + warm-hit cwd-guard + `forget_session`-stays-stash-only, for
  long-lived interactive `serve` sessions. This slice is its foundation.
- `docker pause`/unpause the warm container during the verify/review gap (vs hold-alive).
- Mid-loop cold re-mint degradation (vs `WarmSessionLost` stop).

## Firewall

Designed from the bridge's own seams (`AcpBackend` warm-session, `ContainerRwBackend`/B2a compose+reaper, the
executor's per-node forget, the B2b-3b loop). The retired `bridge-claude` warm-pool (tag
`bridge-claude-retired`/`15f89ac`) is prior-art reference ONLY for the future serve-pool, not lifted here.
Cross-checked by the bridge's own firewalled clean-room `design` workflow (run independently of this spec) +
the dual spec-review (containerized dogfood + a2a-local codex `--agent codex-review`) before the plan.
