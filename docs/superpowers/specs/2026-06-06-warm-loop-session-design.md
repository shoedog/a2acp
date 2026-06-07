# Warm Loop Session — Design (B2b-3c)

**Date:** 2026-06-06
**Status:** Draft (rev2 — folds the firewalled clean-room `design` cross-check; pre dual spec-review).
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
full failure digest. Warm continuity should cut N cold starts AND improve fix quality (the agent reasons from
what it did, not a re-read digest). The live gate showed exactly this multi-cold-turn shape.

## Decisions

1. **Approach A — a loop-owned warm `:rw` session; impl turns bypass the executor; review/verify unchanged.**
   The cross-check sharpened the *why* to **structural impossibility, not mere difficulty** (verified): the
   executor derives `SessionId = workflow-{wf_id}-{node_id}-{run_id}` (`executor.rs:80-85`). The edit turn runs
   the `implement-edit` workflow and each fix runs the `implement-fix` workflow (attempt-qualified `run_id`),
   so their `wf_id`/`node_id` differ → the sessions can **never coincide**, regardless of `forget_session` and
   regardless of any stable `run_id`. Only an **externally-minted, stable `SessionId`** supplied by a bypass
   can be warm. So the loop owns the session directly; review keeps the executor + its `:ro` reviewer DAG;
   verify keeps its toolchain `docker run`. Only the **impl** edit+fix turns move off the executor.

2. **Mechanism: an additive `Warm` lifecycle on the EXISTING `ContainerRwBackend` — NOT a new type.** (Cross-
   check correction: rev1 proposed a separate `WarmContainerSession`; the verified structure lens showed an
   in-place mode reuses the existing RW-target validation, `compose_container_rw`, the `a2a-rw-{owner}-{n}`
   naming, and the spawn-failure reaps WITHOUT forking that logic — the reuse the blind codex lens *asked* for
   but couldn't see.) Add `Lifecycle::{PerTurn, Warm}` + additive `new_warm(cfg, spawn, owner)` (prod) /
   `new_warm_with_hooks(...)` (tests). Extract the existing spawn/compose/configure/spawn-failure-reap block
   (`lib.rs:161-208`) into `async fn open_inner(&self, session, spec) -> Result<WarmInner, BridgeError>`; BOTH
   modes call it and diverge ONLY at the reap trigger. The per-turn path stays **byte-identical**.

3. **Split the conflated `inflight` (the genuinely new mechanism).** Today `inflight` conflates (a) the
   concurrent-turn reject marker, (b) the cancel handle, and (c) the reap trigger, and `ContainerReaper` reaps
   on stream drop. Warm splits these:
   - `warm: Mutex<HashMap<SessionId, WarmInner>>` — **authoritative**; holds the only long-lived
     `Arc<dyn AgentBackend>` + reap handle (`WarmInner { inner, name, reaped, rw_canon }`); entries removed
     **only** at `retire()`.
   - `turn_active: Mutex<HashSet<SessionId>>` — per-turn concurrency marker; cleared on stream end/drop.
   Warm `prompt(session, parts)`: reject if `turn_active` already holds `session` (preserves the existing
   "already in-flight" invariant); else mark. If `warm` has no entry → `open_inner` (spawns the container +
   mints `session/new` ONCE, caches `rw_canon`); else reuse the cached `inner` (re-`configure_session` with
   the cached `rw_canon` — deterministic canonicalization of a stable clone path → the `minted_cwd` guard
   passes; the cache is belt-and-suspenders, risk LOW). `inner.prompt(...)` is wrapped in a lightweight
   **`TurnGuard`** (NOT `ContainerReaper`) that on stream completion OR early drop removes `session` from
   `turn_active` and **does nothing else** — it NEVER reaps; the `warm` cache retains the authoritative `Arc`.

4. **Teardown — `retire()` is the SOLE reap site; there is no meaningful `Drop`.** (Cross-check correction:
   `ContainerRwBackend` has NO `Drop`, and the inner's `:rw` reaper is OFF (`AcpConfig.container = None`,
   `lib.rs:186`) — so dropping the inner does NOT reap the container.) The three leak-safe layers are: explicit
   awaited `retire()` (drain `warm`; per entry `inner.cancel(session)` graceful `session/cancel`, then
   `reap_once`) → a **synchronous `RwSweepGuard`** on drop (the TRUE backstop) → the owner-scoped boot-sweep
   for crash orphans. All reaps idempotent via the shared `reaped` flag. Warm `cancel` cancels the current
   turn but does NOT reap; warm `forget_session` stays stash-only and must NOT touch `warm`.

5. **`RwSweepGuard` — generalize the proven `:ro` precedent.** Generalize `ro_sweep_filter_argv`/
   `ro_sweep_targets` (`main.rs:184-228`) to a `:rw` sibling selecting `AgentKind::ContainerRw` entries;
   `RwSweepGuard` drop → synchronous `a2a-rw-{owner}-` reap. Justified by the same failure mode that forced
   `RoSweepGuard` at the B2b-2 live gate (no `Drop`, inner `:rw` reaper off).

6. **Loop integration — open before the first turn, drive via `ProdEffects`, `retire()` at loop end.** The
   `TweakEffects` trait signature is UNCHANGED (`tweak.rs:146-152`); only `ProdEffects`' bodies + the edit turn
   move off the executor (the fake-executor `run_tweak_loop` tests stay green):
   - `impl_agent_id = edit_graph.nodes[0].agent` (do NOT hard-code `"impl"`); assert both impl workflows are
     single-node (verified G8).
   - `warm = ContainerRwBackend::new_warm(container_rw_cfg_from_entry(impl_entry), AcpContainerSpawn{policy},
     container_owner(.., impl_agent_id))` — config factored out of `make_spawn_fn` (`main.rs:289-299`); the
     spawn seam + owner reused (same owner → sweeps align). `impl_session = SessionId::parse("implement-
     {task_id}")` minted ONCE, pre-commit. `let _rw_guard = RwSweepGuard(rw_targets)` drops AFTER `warm`.
   - **edit turn** = render `edit_graph.nodes[0].prompt_template` (verified inlined from `prompt_file` at
     `config.rs:495-507`) with `{input}=task` → `configure_session` → `drain_turn(warm.prompt(&impl_session,…))`.
   - **`ProdEffects::fix`** = render `fix_template` with the (slimmer) fix input → `configure_session` →
     `drain_turn(impl_backend.prompt(impl_session, parts))` — SAME conversation, SAME container. `ProdEffects`
     gains `impl_backend: &dyn AgentBackend`, `impl_session: &SessionId`, `fix_template: String`.
   - after `run_tweak_loop` returns → `warm.retire().await`; then the hand-off always prints.

7. **`drain_turn` is a NEW helper — `drain_impl` cannot be reused.** (Cross-check catch: the bypass consumes
   raw backend `Update`s, not the executor's `WorkflowEvent`s.) `drain_turn(stream) -> bool` polls the
   `BackendStream` to end; `completed = true` iff an `Update::Done { stop_reason != CANCELLED }` arrived with
   no `Update::Err` (mirrors `executor.rs:140-148` minus the WorkflowEvent layer).

8. **Two-phase fallibility preserved.** `new_warm` + the FIRST edit prompt are PRE-first-commit → keep
   `?`/fail-loud (a mint failure leaves no commit). A warm prompt failing MID-loop (the agent/container died)
   → `drain_turn` returns `false` → the loop classifies `FixIncomplete` (existing) → clean stop + hand-off +
   `RwSweepGuard` reaps. **Strict degradation: NO mid-loop cold re-open** (that would erase the feature + mask
   regressions — both architects independently demanded this). The committed work survives; the hand-off
   always prints.

9. **Fix prompt under continuity — slimmer but SELF-SUFFICIENT.** `build_fix_input` (`tweak.rs:91-123`) drops
   the "it already has your prior commit" framing (same conversation → the agent remembers) but KEEPS a
   one-line task reminder + the verify/review digest + the `git add` / no-`git commit` live-gate mandate.
   Keeping it self-sufficient (correctness does not RELY on memory) is deliberate — it makes a future cold
   re-open (optional slice 3) automatically sound. `prompts/implement-fix.md` reframes from "you have a prior
   commit" to "continue — here is what failed" (keeps the firm MUST-`git add` contract).

10. **Falsifiable acceptance** at the `bridge-container` layer: ONE container spawn across both turns, ZERO
    reaps between turns, the SAME inner, ONE ACP session, ONE reap at `retire` (see Testing). Catches a silent
    regression to per-turn three ways.

## Component / file boundaries

| Layer | File | Change |
|---|---|---|
| **Mechanism** — `Warm` lifecycle (`open_inner` extract, `warm` cache + `turn_active`, `TurnGuard`, warm `prompt`/`cancel`/`retire`) | `crates/bridge-container/src/lib.rs` (`ContainerRwBackend`) | additive `Warm` mode; per-turn path byte-identical |
| **Continuity** — one `session/new`, serialized turns, cwd guard | `crates/bridge-acp/src/acp_backend.rs` (`AcpBackend`) | **unchanged** |
| **Composition root** — build warm backend (off the registry), own `impl_session`, edit turn + `ProdEffects::fix` off-executor, `drain_turn`, `RwSweepGuard` + `rw_sweep_targets`, `container_rw_cfg_from_entry` | `bin/a2a-bridge/src/main.rs` (`implement_cmd`, `ProdEffects`) | edit turn + fix effect rewritten off-executor |
| **Control flow** — verify/review/classify/amend/hand-off | `bin/a2a-bridge/src/tweak.rs` (`run_tweak_loop`, `TweakEffects`) | **unchanged** orchestration; only `build_fix_input` text slims |
| **Reaper** — generalize the `:ro` sweep to `:rw` | `crates/bridge-core/src/reaper.rs` + `main.rs` guards | additive |
| **Prompt** — continuation framing | `prompts/implement-fix.md` | reworded |

## Testing

- **Unit (Docker-free, via the `ContainerSpawn`/`StubInner` seams):** warm `ContainerRwBackend` —
  `warm_reuses_one_inner_and_one_session_across_turns` (extend `StubInner` with `prompts_seen()`/
  `distinct_sessions()`, reuse `CountingSpawn`/`counting_reap`/`noop_sweep`): two prompts on one session →
  `spawn.count == 1` (per-turn regress → 2), `reaps == 0` between turns (regress → ≥1), `inner.prompts_seen()
  == 2`, `distinct_sessions() == 1` (half-warm → 2), then `retire()` → `reaps == 1` (leak-safe). A SECOND test:
  a mid-loop warm-session death classifies as failure and **does NOT** cold-reopen (guards strict degradation,
  which the happy path doesn't cover). Pure `build_fix_input` (slim+self-sufficient), `drain_turn` (Done/Err/
  cancel→completed bool). The B2b-3b fake-executor `run_tweak_loop` tests stay green (seam unchanged).
- **Live gate (Docker, dogfooded):** (1) right-first-try → warm opened, ONE prompt, converged, reaped (0
  leaked). (2) **converge-via-fix with continuity + identity** → an acceptance-orthogonal failure
  (clippy::ptr_arg, per the B2b-3b gotcha) → fix continues the SAME session → assert the SAME container id
  across the edit + fix turns (+ nonzero in the gap), one amended commit, converged. (3) reaper holds (warm
  `:rw` → 0 after `retire`; `:ro` review + verify containers unaffected).

## Build order (slices)

1. **Warm lifecycle in `ContainerRwBackend`** — `new_warm`, extract `open_inner`, `warm` + `turn_active`,
   `TurnGuard`, warm `retire`. Ship with the happy-path + failure-path unit tests. **No bin changes; tested in
   isolation.**
2. **Impl turns off the executor** — `container_rw_cfg_from_entry`, build warm backend + `impl_session`,
   `drain_turn`, rewrite the edit turn + `ProdEffects::fix`, `RwSweepGuard` + `rw_sweep_targets`, slim
   `build_fix_input`, single-node asserts. Review/verify untouched. Live gate here.
3. **(optional, deferred) Idle-death self-heal** — poison-on-error + transparent cold re-open, gated by a
   "dead-inner → re-open, loop continues" test. Only if idle-death is observed against a real container.

## Risks (re-scored after the cross-check)

- **Idle-survival across the verify+review gap (RISKIEST, empirical — the central bet).** The warm `:rw`
  container + its ACP child/stdio sit idle for MINUTES (cargo verify + codex/claude review) between turns. If
  the child or stdio does NOT survive that idle, every attempt-2 fix turn finds a dead session → the loop
  degrades to a single fix (strict stop) until slice 3. **No code reading can settle this** → a quick spike
  against a real `claude-agent-acp` container is warranted before committing to slice 2 (see Owner Decisions).
- **Mechanism complexity** in warm `prompt`/`TurnGuard`/`retire` (splitting the reap trigger from the turn
  marker) — the genuinely new code; contained by the acceptance test + byte-identical per-turn path.
- **Reap leak** — contained by `retire()` + `RwSweepGuard` + `reaped` idempotence + boot sweep.
- **`minted_cwd` false-trip — LOW** (deterministic canonicalization; the `rw_canon` cache is defensive).
- **Continuity-quality unproven (empirical)** — whether warm fixes beat a cold re-prime; gates slice-3
  investment, not the MVP.

## Owner decisions (surfaced by the cross-check)

1. **Idle-death policy = strict-stop (MVP).** If the warm session dies across the gap, fail cleanly with the
   hand-off rather than silently cold-reopening. (Already this spec's choice — confirmed by both architects.)
   The self-heal (slice 3) is gated on actually observing idle-death.
2. **Idle-survival spike BEFORE slice 2.** The whole feature rests on the ACP child surviving a multi-minute
   idle. Recommendation: a quick spike (open a warm `:rw` claude session, idle it minutes, prompt again)
   against a real container first — if it fails, every multi-attempt loop degrades to a single fix until slice
   3, which changes whether slice 2 is worth shipping standalone.
3. **(minor) Measure continuity quality before slice 3** — ship 1+2, measure whether warm fixes beat cold
   re-prime on real runs, then decide slice 3.

## Firewall

Designed from the bridge's own seams; the retired `bridge-claude` warm-pool (tag `bridge-claude-retired`/
`15f89ac`) is prior-art reference ONLY for the future serve-pool, not lifted here. Cross-checked by the
bridge's own firewalled clean-room `design` workflow (run independently of this spec — it converged on the
spine blind and corrected rev1's new-type→in-place-mode, the `inflight` split, the no-`Drop`/`RwSweepGuard`
teardown, the `drain_turn` raw-`Update` catch, and the structurally-impossible-on-executor *why*). Dual
spec-review next (containerized dogfood + a2a-local codex `--agent codex-review`) before the plan.
