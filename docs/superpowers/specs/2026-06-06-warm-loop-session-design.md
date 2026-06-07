# Warm Loop Session — Design (B2b-3c)

**Date:** 2026-06-06
**Status:** Draft (rev3 — folds the dual spec-review: containerized dogfood primary, claude-soundness lens
verified + codex-rigor lens, + a2a-local codex backstop; both needs-changes). Idle-survival spike PASSED.
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
   The *why* is **structural impossibility** (verified): the executor derives `SessionId =
   workflow-{wf_id}-{node_id}-{run_id}` (`executor.rs:80-85`); edit runs the `implement-edit` workflow and each
   fix runs `implement-fix` (attempt-qualified `run_id`), so their `wf_id`/`node_id` differ → the sessions can
   never coincide, regardless of `forget_session` or any stable `run_id`. Only an externally-minted, stable
   `SessionId` supplied by a bypass can be warm. Review keeps the executor + its `:ro` reviewer DAG; verify
   keeps its toolchain `docker run`. Only the **impl** edit+fix turns move off the executor.

2. **Mechanism: an additive `Warm` lifecycle on the EXISTING `ContainerRwBackend`, with the divergence
   contained to ONE injected reap-trigger.** Add `new_warm(cfg, spawn, owner)` (prod) / `new_warm_with_hooks`
   (tests). Extract the existing spawn/compose/configure/spawn-failure-reap block (`lib.rs:161-208`) into
   `async fn open_inner(&self, session, spec) -> Result<WarmInner, BridgeError>` (spawn + canonical configure
   ONLY — `session/new` is lazy inside `AcpBackend::prompt`). **(Dual-review BLOCKER-3, code-verified):** an
   in-place mode otherwise makes the struct carry TWO lifecycle state machines (per-turn `inflight`
   Reserving/Live + reap-on-stream-drop; warm `warm`+`turn_active` + never-reap-except-`retire`), and a future
   per-turn edit could silently break warm's never-reap invariant. So the divergence is **a single injected
   reap policy** (the turn-end guard constructor): `prompt`/`cancel`/`retire` do NOT each branch on a
   `Lifecycle` enum — they take the reap behavior as data. Per-turn injects "reap on stream end"; warm injects
   "clear `turn_active` only, never reap". (A separate type that delegates to `open_inner` is the alternative;
   rejected because it re-implements the full `AgentBackend` trait surface + plumbing for no extra safety once
   the reap policy is injected.) The per-turn path stays **behaviorally identical** — same spawn args, same
   configure behavior, same reap-on-drop, existing per-turn tests preserved (NOT literal byte-identity).

3. **Two cache structures + the reuse-turn error seam (the subtlest correctness point — dual-review BLOCKER-1
   & 2, verified).** Split the conflated `inflight`:
   - `warm: Mutex<HashMap<SessionId, WarmInner>>` — **authoritative**; the only long-lived `Arc<dyn
     AgentBackend>` + reap handle (`WarmInner { inner, name, reaped, rw_canon }`); entries removed **only** at
     `retire()`.
   - `turn_active: Mutex<HashSet<SessionId>>` — per-turn concurrency marker.
   Warm `prompt(session, parts)`:
   1. reject if `turn_active` already holds `session`; else insert **and immediately wrap in an RAII
      `TurnGuard`** (so cleanup never depends on reaching the stream — covers every early-return/error path).
   2. if `warm` has no entry → `open_inner` (spawns + caches `rw_canon`); a spawn/configure failure here is
      PRE-first-commit on the edit turn → reap the just-opened container + remove, clear `turn_active`, return
      `Err` (fail-loud).
   3. else (REUSE / fix turn) reuse the cached `inner`; re-`configure_session` with the cached `rw_canon`.
   4. `inner.prompt(...)` — note `AgentBackend::prompt` itself returns `Result` and `session/new` is lazy, so
      it can `Err` **before any stream exists**.
   **The reuse-turn error rule (BLOCKER-1):** on a REUSE turn, EVERY non-stream error (`configure_session`
   `Err`, `inner.prompt(...)` `Err`) **clears `turn_active`, does NOT reap, and returns `Err`** — the warm
   cache keeps the authoritative `Arc` so a *transient* fix-turn error can't nuke the warm container. The
   loop's `TweakEffects::fix -> bool` (`tweak.rs:151`, type-forbids `?`-propagation) turns that `Err` into
   `completed=false` → `FixIncomplete` → strict stop + hand-off; `RwSweepGuard` does the eventual reap. (The
   per-turn analogs at `lib.rs:204-227` REAP — that behavior must NOT be copied into the warm reuse arm.)
   **`turn_active` lifecycle (BLOCKER-2):** the `TurnGuard` clears `turn_active` on stream end OR drop and
   does nothing else (never reaps); warm `cancel` cancels the current turn AND clears `turn_active`
   (idempotent with the guard); `retire()` while a turn is active = cancel-then-reap (define: it cancels the
   in-flight turn, then reaps). Warm `forget_session` stays stash-only and must NOT touch `warm`.

4. **Teardown — `retire()` is the SOLE reap site; there is no meaningful `Drop`.** (`ContainerRwBackend` has
   no `Drop`, the inner's `:rw` reaper is OFF, `lib.rs:186` — dropping the inner does NOT reap.) Layers:
   explicit awaited `retire()` (drain `warm`; per entry `inner.cancel(session)` then `reap_once`) → a
   synchronous `RwSweepGuard` on drop (the true backstop) → the owner-scoped boot-sweep. Idempotence note
   (corrected): `retire()`/`cancel` reaps are **flag-idempotent** (shared `reaped`); the `RwSweepGuard` sweep
   is **name-based** (list-then-`rm -f`, like `ro_sweep` `main.rs:207-218`) and **independently** idempotent —
   it does NOT consult `reaped`.

5. **`RwSweepGuard` — shared-owner invariant + declaration order + helper home (dual-review M8).**
   - **Shared owner (silent-leak guard):** the warm backend's spawn-time container owner AND the
     `RwSweepGuard` target owner MUST be computed from the SAME triple `(config_path, sb.mount, agent_id)` via
     ONE shared helper (assert equal). Otherwise the guard sweeps a different owner and MISSES the warm
     container.
   - **Declaration order (corrected — rev2 had it backwards):** Rust drops locals in REVERSE declaration
     order, so `_rw_guard` must be declared **BEFORE** `warm` (and before any registry-owned backends) so it
     drops **after** them — mirroring `_ro_guard` (`main.rs:752`, declared before the registry `:756`).
   - **Home:** add `rw_sweep_filter_argv` as a sibling of `ro_sweep_filter_argv` in **`bridge_core::sandbox`**
     (`sandbox.rs:146`); keep `rw_sweep_targets` + `RwSweepGuard` in `main.rs` (beside `ro_sweep_targets`).

6. **Loop integration + two-phase fallibility (dual-review M4/M5/M9).**
   - **Config identity (M9):** resolve `impl_agent_id = edit_graph.nodes[0].agent`; assert BOTH `implement-edit`
     and `implement-fix` are single-node AND reference the **same** agent id AND that entry is
     `AgentKind::ContainerRw` AND its container config matches — else **fail loud pre-first-commit** (the warm
     backend is built from the edit entry and drives BOTH turns on ONE container, so a divergent fix agent
     would be silently ignored).
   - **`SessionId` contract (M5):** `impl_session = SessionId::parse("implement-{task_id}")` minted ONCE,
     pre-commit; `task_id` is the existing fs/branch-safe id (`impl-<pid>-<nonce>`), so the parse is a
     formality — but state it: a parse failure is pre-first-commit → fail-loud.
   - **Order (M4):** the warm prompts: edit = render `edit_graph.nodes[0].prompt_template` (verified inlined
     from `prompt_file` at `config.rs:495-507`) with `{input}=task` → `configure_session` →
     `drain_turn(warm.prompt(&impl_session,…))`; on the edit `prompt(...).await` returning `Err` (pre-stream)
     → fail-loud (pre-commit). `ProdEffects::fix` = render `fix_template` → `configure_session` →
     `drain_turn(impl_backend.prompt(impl_session, parts))` (reuse-turn error rule from §3). After
     `run_tweak_loop` returns: **compute hand-off → print hand-off → `let _ = warm.retire().await`** (retire is
     fallible; the error is LOG-ONLY and never alters the command result; a test proves a retire error still
     prints the hand-off). `ProdEffects` gains `impl_backend: &dyn AgentBackend`, `impl_session: &SessionId`,
     `fix_template: String`. The `TweakEffects` trait signature is UNCHANGED (`tweak.rs:146-152`).

7. **`drain_turn` — STRICTER than the executor, NOT a mirror (dual-review M6, verified).** The executor leaves
   `ok=true` on a clean `None` (`executor.rs:148`) — a false success — and `ports.rs` has no `Update::Err`
   (failures are stream-level `Some(Err(_))`). So: `drain_turn(stream) -> bool` is **complete iff
   `Some(Ok(Update::Done { stop_reason != CANCELLED }))` arrived; `Some(Err(_))` and a clean `None` →
   incomplete.** Outcome table (each → completed-bool): single `Done(end_turn)`→true; `Done(cancelled)`→false;
   `Err` (transport/stream) before `Done`→false; stream end (`None`) without `Done`→false; multiple `Done`s →
   first terminal wins (true if non-cancelled); unknown stop reason → true iff != CANCELLED. (Do NOT cite "it
   mirrors `executor.rs`" — the implementer must not reintroduce the executor's clean-`None`→true.)

8. **Fix prompt under continuity — slimmer but SELF-SUFFICIENT.** `build_fix_input` (`tweak.rs:91-123`) drops
   the "it already has your prior commit" framing (same conversation) but KEEPS a one-line task reminder + the
   verify/review digest + the `git add` / no-`git commit` mandate (correctness does NOT rely on memory → a
   future cold re-open stays sound). `prompts/implement-fix.md` reframes to "continue — here is what failed".

## Component / file boundaries

| Layer | File | Change |
|---|---|---|
| **Mechanism** — `new_warm`, `open_inner` extract, `warm` cache + `turn_active`, injected reap-trigger (`TurnGuard` for warm / reap-on-drop for per-turn), warm `prompt`/`cancel`/`retire` | `crates/bridge-container/src/lib.rs` (`ContainerRwBackend`) | additive `Warm`; per-turn behaviorally identical |
| **Continuity** — one `session/new`, serialized turns, cwd guard | `crates/bridge-acp/src/acp_backend.rs` | **unchanged** |
| **Pure reap-filter argv** — `rw_sweep_filter_argv` (sibling of `ro_sweep_filter_argv`) | `crates/bridge-core/src/sandbox.rs` | additive |
| **Composition root** — `container_rw_cfg_from_entry`; build warm backend (off-registry) from the edit entry + config-identity asserts; `impl_session`; `_rw_guard` declared BEFORE `warm` (shared owner); edit turn + `ProdEffects::fix` off-executor; `drain_turn`; `rw_sweep_targets`/`RwSweepGuard`; hand-off→print→`let _ = retire()` | `bin/a2a-bridge/src/main.rs` (`implement_cmd`, `ProdEffects`) | edit + fix effects rewritten off-executor |
| **Control flow** — verify/review/classify/amend/hand-off | `bin/a2a-bridge/src/tweak.rs` (`run_tweak_loop`, `TweakEffects`) | **unchanged**; only `build_fix_input` text slims |
| **Reaper primitives** | `crates/bridge-core/src/reaper.rs` | reused |
| **Prompt** | `prompts/implement-fix.md` | reworded |

## Testing

- **Unit (Docker-free, via `ContainerSpawn`/`StubInner`):**
  - `warm_reuses_one_inner_and_one_session_across_turns` — two prompts on one session → `spawn.count == 1`
    (per-turn regress → 2), `reaps == 0` between turns (regress → ≥1), same `inner`, and **observable
    `session/new` call-count == 1** (M7: `distinct_sessions()==1` is too weak — assert the one-time backend
    init/`session/new` count, falsifiable against repeated `session/new` with the same id), then `retire()` →
    `reaps == 1`.
  - **reuse-turn error does NOT reap** (BLOCKER-1): inject a fix-turn `inner.prompt` `Err` → `warm` entry
    retained, `reaps == 0`, `turn_active` cleared, the call returns `Err` → (at the loop layer) `FixIncomplete`.
  - **edit-turn pre-stream error reaps + fails loud** (open_inner / first-prompt `Err`).
  - **`turn_active` lifecycle**: a second concurrent prompt on an active session is rejected; `cancel` clears
    `turn_active`; `retire()` while active cancels-then-reaps.
  - **`drain_turn` outcome table** (§7) incl. clean-`None`→false (the executor-divergence guard).
  - **config-identity asserts** (edit/fix same single-node ContainerRw agent → else reject).
  - pure `build_fix_input` (slim+self-sufficient). The B2b-3b fake-executor `run_tweak_loop` tests stay green.
- **Bin-layer (fake backend):** a `retire()` that returns `Err` STILL prints the hand-off (M4 invariant).
- **Live gate (Docker, dogfooded):** (1) right-first-try → warm opened, ONE prompt, converged, reaped (0
  leaked). (2) **converge-via-fix with continuity + identity** → acceptance-orthogonal failure
  (clippy::ptr_arg) → fix continues the SAME session → assert the SAME container id across edit + fix (+
  nonzero in the gap), one amended commit, converged. (3) reaper holds (warm `:rw` → 0 after `retire`).

## Build order (slices)

1. **Warm lifecycle in `ContainerRwBackend`** — `new_warm`, `open_inner`, `warm`+`turn_active`, the injected
   reap-trigger, warm `prompt`/`cancel`/`retire`. Ship with all the unit tests above (incl. the reuse-turn-no-
   reap + turn_active-lifecycle + drain-table). No bin changes; tested in isolation.
2. **Impl turns off the executor** — `container_rw_cfg_from_entry`, `rw_sweep_filter_argv` (sandbox) +
   `rw_sweep_targets`/`RwSweepGuard` (main, declared before `warm`, shared owner), build the warm backend +
   `impl_session` + config-identity asserts, `drain_turn`, rewrite the edit turn + `ProdEffects::fix`,
   hand-off→print→`let _ = retire()`, slim `build_fix_input`. Review/verify untouched. Live gate here.
3. **(optional, deferred) Idle-death self-heal** — poison-on-error + transparent cold re-open, gated by a
   "dead-inner → re-open, loop continues" test. Only if idle-death is observed (the spike says it isn't, at 7m).

## Risks (re-scored)

- **Idle-survival across the verify+review gap — VALIDATED.** Spike (committed:
  `docs/superpowers/spikes/2026-06-07-warm-session-idle-survival.md`): real `:rw` `claude-agent-acp`,
  initialize → session/new → prompt → **420s idle** → re-prompt the SAME session → **PASS** (container alive,
  `end_turn`). CAVEAT: a pathological gap (cold cargo verify + slow review) could exceed 7 min; that tail (+ a
  mid-loop crash) is covered by strict-stop (§3) + the optional slice-3 self-heal.
- **Reuse-turn error handling** (the genuinely subtle code): must clear-`turn_active`/no-reap/return-`Err`
  (§3) — contained by the dedicated unit test.
- **Reap leak / owner mis-alignment** — contained by the shared-owner invariant (§5) + `retire()` +
  `RwSweepGuard` + boot-sweep.
- **`minted_cwd` false-trip — LOW** (deterministic canonicalization; `rw_canon` cache defensive).
- **Continuity-quality unproven (empirical)** — gates slice-3 investment, not the MVP.
- **Operational (out of scope): containerized claude OAuth token TTL.** The a2a-creds mount is a static copy
  that goes stale ~hourly (the host token auto-refreshes; the mount does not) → containerized claude workflows
  fail until re-copied. Not a B2b-3c concern, but worth a small follow-up (mount the host creds file directly,
  or a copy-before-run step).

## Owner decisions

1. **Idle-death policy = strict-stop (MVP).** Locked; both architects + the dual review agree.
2. **Idle-survival spike — DONE, PASS** (7-min idle survived). Slice 2 greenlit; slice 3 deferred.
3. **In-place `Warm` mode with a single injected reap-trigger** (not a separate type, not `Lifecycle`-branching
   inside `cancel`/`retire`) — resolves the dual review's BLOCKER-3 "decide before planning". Surfaced here for
   visibility; the plan locks the exact injection shape.

## Firewall

Designed from the bridge's own seams; the retired `bridge-claude` warm-pool (tag `bridge-claude-retired`/
`15f89ac`) is prior-art reference ONLY for the future serve-pool. Cross-checked by the bridge's own firewalled
clean-room `design` workflow (independent of this spec) + the dual spec-review (containerized dogfood — claude
soundness lens verified against code + codex rigor lens — and the a2a-local codex backstop). rev3 folds: the
reuse-turn error seam (no-reap), the full `turn_active` lifecycle, the single-injected-reap-trigger containment,
the corrected drop order + shared-owner invariant + `sandbox.rs` helper home, the retire-ordering/fallibility,
the `SessionId`/config-identity contracts, the STRICTER-than-executor `drain_turn` table, the falsifiable
`session/new`-count test, and the committed spike evidence. Plan next.
