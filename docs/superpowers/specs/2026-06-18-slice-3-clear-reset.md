# Slice 3 — Clear / reset — design spec

**Status:** design (2026-06-18). Fourth orchestration slice. Governed by
`2026-06-17-orchestration-slicing.md` (Slice 3) over the converged architecture
(`2026-06-17-orchestration-architecture.md`: OPEN-4, DIVERGENCE-1, GENERATION-MONOTONICITY). Builds on
Slice 0 (`SessionManager` + `release_session` + the generation-suffixed `backend_session`), Slice 1
(reconcile + the `Reconciling`/`Expiring` claim discipline), Slice 2 (`record_usage` + `WarmTurn`).
ACP grounding: `docs/references/acp-protocol-v1.md`.

## Goal

Give the orchestrator a **`clear`** lever: reset a warm session's CONTEXT to empty **while keeping the process
warm** (no cold respawn). `clear` on a `contextId` drops the agent's conversation memory so the next turn
starts fresh — without paying the ≈27s cold-start. This is the standalone half of the A4 "compact/clear"
ask; **compact (S4) composes on top** (summarize → clear → seed). It is the precondition for a long-lived warm
session that an orchestrator can recycle instead of releasing.

## The core problem (code-grounded) — why clear needs a NEW generation, not an in-place reset

- **`forget_session` does NOT reset context.** It only drops the per-session config stash; today's "freshness"
  is the *accident* of the executor minting a fresh bridge `SessionId` per node (SPIKE-A). For a warm handle
  the `backend_session` id is STABLE, so forgetting it does not give a fresh ACP `session/new`.
- **The ACP session's identity is `OnceCell`-pinned and NON-resettable.** `AgentSession.agent_id:
  OnceCell<AgentSessionId>` (`acp_backend.rs:275`) + `minted_cwd: OnceCell<String>` (`:283`) are set exactly
  once by the `session/new` that `ensure_session` drives (`get_or_try_init`). You CANNOT re-init them in place.
- **⇒ DIVERGENCE-1 (converged ruling): clear = a NEW bridge `SessionId` per generation + release the old.**
  Bump the handle's `generation`, mint a generation-scoped `backend_session` (`ctx-{ctx}-g{N}`), `release` the
  old bridge session, `configure` the new one; the next turn's `ensure_session` hits the existing `session/new`
  fresh-mint path (SPIKE-A-proven) → **fresh context on the SAME warm process**. **Zero new minting code** — it
  reuses Slice-0 `release_session` + `configure_session` + the lazy mint. An in-flight turn holds the OLD
  `Arc`/`turn_lock`/routing-sender, so a fresh id (not a stable-key swap) keeps stale writes trivially isolable.

## Decisions (settled by the architecture; carried in)

1. **`clear` == the reset primitive.** There is one operation: reset the context to a new generation. The
   user-facing `session/clear` calls it. (`compact` in S4 = summarize → this reset → seed; not in Slice 3.)
2. **Generation-scoped `backend_session`:** `ctx-{ctx}-g{N}` (the `g0` suffix already exists,
   `session_manager.rs:279`). `generation` is already on `WarmHandle` (`:38`, `0` at mint); clear increments it.
3. **Require `Idle`; `force` cancels first.** clear on a `Running` handle is rejected (`HandleBusy`) unless
   `force` — which cancels the in-flight turn, then resets. (Architecture: "require `Idle` unless `force_cancel`".)
4. **GENERATION-MONOTONICITY (load-bearing):** a turn captured at generation N must never mutate a handle that
   has since advanced to N+1. A stale (force-cancelled) old-generation turn's late `finish_turn`/`record_usage`
   is **dropped**, never applied to the new generation.
5. **Keep the process + lease + handle identity warm.** clear drops only the bridge *session* (context); the
   shared ACP process, the registry lease, the `SessionHandleId`, and the warm-table entry all persist (clear ≠
   release). For `ContainerRwBackend`, `release_session(old)` reaps the old turn's `:rw` container; the next
   turn mints a fresh one (same warm-mode policy).

## v2 — dual spec-review fixes folded (codex-xhigh + Opus, both `fix-then-ship`)

Both lenses converged (same force-race + wire-name) and ruled UNANIMOUSLY on D1–D4. These resolutions are
BINDING and SUPERSEDE conflicting detail below. **No redesign** — composition + generation-thread + `clear`
verb all confirmed; the gaps are force-path concurrency + a fallible-await strand + the wire name.

- **FIX-1 (BLOCKER, both) — the wire method is `SessionClear`, NOT `session/clear`.** The shipped session
  surface uses CamelCase, slash-free method names — `"SessionStatus"`/`"SessionRelease"`/`"SessionCancel"`
  (`server.rs:672-681`); a `session/clear` registration would 404 (`JSONRPC_METHOD_NOT_FOUND`) and break every
  DoD that invokes it. **Use `"SessionClear"`** (sibling in the same dispatch match), params `{contextId,
  force?}`, result `{contextId, cleared, generation}`. The **CLI verb stays `session clear`** (CLI↔wire names
  are independent — `session cancel`↔`SessionCancel` already pairs this way; `main.rs:2724-2737`); add `clear`
  to the CLI help (`main.rs:104`). (Same class as the [[streaming-reattach-shipped]] `id`-vs-`taskId` gotcha.)
- **FIX-2 (BLOCKER, both) — the `force` path must claim BEFORE cancel; never reuse `SessionManager::cancel`.**
  The shipped `cancel` sets the handle `Idle` *before* awaiting the backend cancel (`session_manager.rs:
  370-386`), and `AcpBackend::cancel` is prompt (does NOT await turn teardown, `acp_backend.rs:~1879`). So a
  force-clear that cancels-then-claims exposes the OLD generation to a concurrent `checkout_turn` (the Slice-1
  ABA). **Fix:** on `Running + force`, transition **`Running → Resetting` ATOMICALLY under the `by_context`
  lock** (never bounce through `Idle`, never call `SessionManager::cancel`); THEN best-effort `backend.cancel
  (old_id)` + `release_session(old_id)` under the claim. The cancelled producer's late `tx.send`/`record_usage`
  target `old_id` (about to be released) → inert; say so rather than "confirm torn down".
- **FIX-3 (BLOCKER, both) — the generation guard needs a STATE predicate.** `gen == handle.generation` alone
  is insufficient *during* the `Resetting` window (generation is still `old_gen` until commit), so a stale
  `finish_turn`/`record_usage` would mutate the resetting handle. **Fix:** `finish_turn(ctx, gen)` /
  `record_usage(ctx, gen, snap)` apply **only if `gen == handle.generation` AND `handle.state == Running`**;
  on ANY mismatch (stale generation OR a claim state `Resetting`/`Reconciling`/`Expiring`) they are a
  **read-only NO-OP — touch NOTHING** (not `state`, not `op`, not `last_used`, not `usage`). Key solely on
  `(ctx, generation)`; **ignore `op`** (it is task-derived `op-{task}` → a force-clear + same-task follow-up
  can collide on `op`, so it is not a safe discriminator).
- **FIX-4 (MAJOR — blocking, codex) — fallible `configure_session` must not strand the handle.**
  `configure_session` returns `Result` (`ports.rs:42`); §Arch step 5's `.await?` before the re-acquire would
  leave the handle permanently `Resetting`. **Fix (mirror the Slice-1 reconcile non-clean path,
  `session_manager.rs:214-268`):** reconstruct the `SessionSpec` BEFORE claiming; run `release_session(old)`
  + `configure_session(new)` into **captured `Result`s** (no `?` across the dropped lock); ALWAYS re-acquire +
  revalidate the exact claim; on a clean outcome commit, on ANY error/cancel/release-in-window **EXPIRE**
  (remove handle + drop lease) and return the original error.
- **FIX-5 (MAJOR, Opus) — `force` deliberately skips the architecture's "drained `Canceling`" precondition,
  because `release_session(old)` IS the drain.** ACP `release_session` re-cancels + drops `sessions[id]`
  (`acp_backend.rs:~2048`); ContainerRw `release_warm` cancels + reaps the container
  (`bridge-container/src/lib.rs:424`). The old generation is discarded, so a partially-drained old turn cannot
  corrupt anything — state this so a reviewer-against-DIVERGENCE-1 doesn't flag a deviation.
- **FIX-6 (MAJOR, Opus) — name the fingerprint-superset invariant.** Reconstructing `SessionSpec` from the
  `fingerprint` is lossless TODAY (`fp.config: EffectiveConfig` carries `model/effort/mode`,
  `domain.rs:161-165`; `cwd: Option<String>` round-trips via `SessionCwd::parse`). Add the invariant:
  **"the `SessionSpecFingerprint` must remain a SUPERSET of `SessionSpec`'s configurable fields; the
  reconstruction is only safe while that holds"** — insurance against a future field added to `SessionSpec.
  config` but not the fingerprint.
- **FIX-7 (MINOR — Slice-1 ABA backstop, both) — the `Resetting` deferral-sites CHECKLIST.** Every site that
  special-cases `Reconciling`/`Expiring` must ALSO handle `Resetting`, or the closed ABA/release-reuse races
  reopen: (a) `status()` match (`:324-329`, compiler-enforced); (b) the `checkout_turn` busy-check (`:161-164`
  — `Resetting` ⇒ `HandleBusy`); (c) `release` deferral (`:~356` — set `expire_after_reconcile`); (d) `cancel`
  deferral (`:~378`). The PLAN must enumerate all four.
- **FIX-8 (MINOR) — `SessionGeneration` has no increment helper** (`new`/`get` only, `ids.rs:41`). Use
  `SessionGeneration::new(h.generation.get() + 1)` (or add a `next()` method) — avoid a bare-`u64` local drift.
- **FIX-9 (MINOR, Opus) — SEQ-AUTHORITY:** `SessionClear` on a context that has only a DETACHED task (no warm
  handle in `by_context`) returns `SessionNotFound` (clean), not a surprise.
- **FIX-10 (MINOR, Opus) — cwd parse-failure → `BridgeError::ConfigInvalid{reason}`** (`error.rs:69`,
  `client_message`-safe), NOT `InvalidRequest` (not client-caused) and NOT `unwrap()`.
- **FIX-11 (MINOR, Opus) — pin `UsageSnapshot::default()` = `used: None, size: None`** (null, not `Some(0)`):
  DoD-3 asserts the wire `windowFraction` is `null` after clear, and the degrade paths
  (`session_manager.rs:80`) rely on `None`.

**D1–D4 — both lenses UNANIMOUS:** D1 **composition** (no new backend method) + the FIX-4 cleanup path; D2
**include `force`** (claim-before-cancel, FIX-2); D3 **explicit generation thread** + the FIX-3 state guard;
D4 **`clear`** verb / `reset_session` primitive / **`SessionClear`** wire (FIX-1). **Scope: in-bounds, no
creep.** No-regression conditional on FIX-2/3/4/7; live-gate provable once FIX-1 lands.

## Findings (grounded in the code)

- **`release_session` is shipped (Slice 0) and does the right teardown.** `AgentBackend::release_session`
  (`ports.rs:55`) defaults to `forget_session`; **`AcpBackend` overrides it to remove BOTH `session_cfg[id]`
  AND `sessions[id]`** (so the OnceCell-pinned `AgentSession` is dropped); **`ContainerRwBackend::release_session`
  (`bridge-container/src/lib.rs:553`) reaps that one session's container** via `release_warm` (`:424`). So
  clear can RELEASE the old generation through the existing trait method — no new backend method needed for the
  teardown half.
- **`configure_session` stashes the per-session spec for the lazy mint** (`ports.rs:42`). The mint branch calls
  it (`session_manager.rs:283`); the RESUME branch does NOT. So after clear, the new id needs an explicit
  `configure_session(new_id, spec)` or the next `ensure_session(new_id)` mints with no config (model/effort/cwd
  lost). **Clear must `configure_session` the new id.**
- **The handle stores a `fingerprint`, not a `SessionSpec`.** `SessionSpecFingerprint { agent, config:
  EffectiveConfig, cwd: Option<String> }` (`:39`). Clear reconstructs the `SessionSpec { config: fp.config,
  cwd: fp.cwd → SessionCwd::parse }` to re-`configure_session` — same effective config (clear changes context,
  NOT config).
- **The claim-during-async-window pattern already exists** (Slice 1): `Reconciling`/`Expiring` own the handle
  across a dropped-lock `await` so a concurrent `checkout` is `HandleBusy` and a concurrent `cancel`/`release`
  defers (`expire_after_reconcile`). Clear reuses this shape with a new `Resetting` state.
- **`finish_turn`/`record_usage` are `ctx`-keyed, NOT generation-aware** (`:313`, `:343`). A force-cancelled
  old-generation turn's producer will still call `finish_turn(ctx)`/`record_usage(ctx,..)` after the reset →
  it would flip the NEW generation to `Idle` / overwrite the fresh-zero usage. **This is the
  GENERATION-MONOTONICITY hole; clear closes it.**
- **`WarmTurn`** (`:52`) carries `backend`+`session`+`usage_warning`; the producer holds it via `WarmTurnGuard
  { sm, ctx }` (`server.rs:452`) whose Drop calls `finish_turn(ctx)`. Generation-scoping requires threading the
  captured generation through `WarmTurn` → `WarmTurnGuard` → `finish_turn`/`record_usage`.

## Architecture

### 1. `SessionManager::reset_session(ctx, ResetOpts)` (the primitive)

```rust
pub struct ResetOpts { pub force: bool }     // force = cancel a Running turn first; default require Idle
pub enum ResetOutcome { Cleared { generation: u64 }, NotFound }

pub async fn reset_session(&self, ctx: &ContextId, opts: ResetOpts) -> Result<ResetOutcome, BridgeError>
```
Algorithm (mirrors the Slice-1 claim discipline):
1. Lock `by_context`. Missing ctx → `Ok(NotFound)`.
2. State gate: `Idle` → proceed to claim. `Running` → if `!force` return `HandleBusy`. `Reconciling`/
   `Expiring`/`Resetting` → `HandleBusy` (another lifecycle op owns the handle).
3. **Claim (FIX-2 — atomic, no `Idle` bounce):** under the SAME `by_context` lock hold, transition the handle
   directly to `Resetting` — from `Idle`, OR from `Running` when `force` (NEVER via `Idle`, NEVER by calling
   `SessionManager::cancel` which idles-before-await and reopens the ABA). Reconstruct the `SessionSpec` from
   the `fingerprint` HERE (before dropping the lock), capture `old_id`, `claimed_id`, `new_gen =
   SessionGeneration::new(generation.get()+1)`, `new_id = SessionId("ctx-{ctx}-g{new_gen}")`. `Resetting`
   makes checkout→`HandleBusy` and cancel/release→defer (`expire_after_reconcile`). `drop(lock)`. For
   `force`, the in-flight turn is torn down by `release_session(old_id)` itself (FIX-5) — best-effort
   `backend.cancel(old_id)` first is fine but not required; the old producer's late writes target `old_id`
   (being released) → inert.
4. `backend.release_session(&old_id).await` (drop old context / reap old `:rw` container; FIX-5: this is also
   the `force` drain).
5. **(FIX-4 — capture, do NOT `?`)** `let cfg = backend.configure_session(&new_id, &spec).await;` — hold the
   `Result`; NEVER early-return across the dropped lock (an early `?` would strand the handle as `Resetting`).
6. Re-acquire lock + **re-validate the exact claim** (handle present, `id == claimed_id`, state ==
   `Resetting`); else `SessionExpired`. **Non-clean** — `cfg.is_err()` OR a concurrent `release`/`cancel`
   flagged `expire_after_reconcile` during the window → **EXPIRE** (remove handle + drop lease) and return the
   original `cfg` error (config case) / `SessionExpired` (cancel/release case) — exactly the Slice-1 reconcile
   non-clean path. **Clean** → commit: `backend_session = new_id`, `generation = new_gen`, **`usage =
   UsageSnapshot::default()`** (FIX-11: `used:None`/`size:None` — fresh context = fresh budget, null window),
   `state = Idle`, `last_used = now`. Return `Cleared { generation: new_gen }`.
7. The NEXT `checkout_turn(ctx)` is the RESUME path; the fingerprint is unchanged (clear preserves config) so
   it dispatches against `new_id`; the first prompt's `ensure_session(new_id)` mints a fresh `session/new` →
   **fresh context, warm process** (SPIKE-A).

**`reset_session` is SessionManager composition over the shipped `release_session` + `configure_session`** — no
new `AgentBackend` trait method (DIVERGENCE-1's "zero new minting code"). *(Considered: a dedicated atomic
`AgentBackend::reset_session` that releases-then-configures under one backend lock — deferred unless the review
finds the two-call window unsafe; the `Resetting` claim already serializes per-handle, and a fresh id makes a
partial state harmless.)*

### 2. GENERATION-MONOTONICITY — generation-scoped turn completion

- `WarmTurn` gains `pub generation: SessionGeneration` (captured at `checkout_turn` from `h.generation`).
- `WarmTurnGuard { sm, ctx, generation }` (server.rs) carries it; its Drop calls `finish_turn(&ctx,
  generation)`. The Slice-2 usage tap calls `record_usage(&ctx, generation, snap)`.
- `finish_turn(ctx, gen)` / `record_usage(ctx, gen, snap)`: **(FIX-3)** apply **only if `gen ==
  handle.generation` AND `handle.state == Running`** — a turn only legitimately completes/idles a *Running*
  handle. On ANY mismatch (stale generation, OR a claim state `Resetting`/`Reconciling`/`Expiring` even at the
  same generation), they are a **read-only NO-OP — mutate NOTHING** (not `state`, not `op`, not `last_used`,
  not `usage`). Key solely on `(ctx, generation)`; **ignore `op`** (task-derived → unsafe discriminator on a
  force-clear + same-task follow-up). This is the entire stale-write guard — it closes the `Resetting`-window
  hole that `gen`-alone left open.

### 3. `clear` surface: JSON-RPC `session/clear` + CLI

- **`SessionClear { contextId, force? }`** → `{ contextId, cleared: true, generation }` (or `SessionNotFound`).
  **(FIX-1: the wire method is `"SessionClear"` — CamelCase/slash-free — sibling to the shipped
  `"SessionStatus"`/`"SessionRelease"`/`"SessionCancel"` in the `server.rs:672-681` dispatch match; NOT
  `session/clear`.)** Maps `reset_session`; `force` defaults false.
- **CLI:** extend `session status|release|cancel` (`main.rs`) to `session clear <contextId> [--force]` (CLI verb
  is `clear`; wire method is `SessionClear`) + the `main.rs:104` help line.

## Scope

**IN:** `SessionManager::reset_session` (new-generation reset: release old + configure new + bump generation +
reset usage, `Resetting` claim discipline, require-Idle-unless-`force`); the `Resetting` `SessionState` +
status string; `ResetOpts`/`ResetOutcome`; **GENERATION-MONOTONICITY** (generation-scoped `WarmTurn` +
`finish_turn`/`record_usage` no-op on stale gen, threaded through `WarmTurnGuard`); `session/clear` JSON-RPC +
CLI `session clear`. Reuses the shipped `release_session` (ACP + ContainerRw) + `configure_session` + lazy mint.

**OUT (later slices):** `compact` (S4 — summarize → reset → seed-as-PrependNextTurn); a dedicated atomic
backend `reset_session` method (composition suffices); `run-workflow --serve --context` keep-warm (S5); the
rich journal/seq (S6 — Slice 3's "seq" guard is the generation no-op, not a journal cursor); MCP (S8);
post-restart `session/load` rehydration. **No** change to release/cancel semantics; **no** auto-clear.

## Definition of Done + LIVE-GATE (real serve + real codex)

1. **Clear drops context:** `submit --context C` "remember the codeword ZEBRA"; `session clear C`; a follow-up
   `submit --context C` "what was the codeword?" → the agent does **NOT** know it (fresh context). vs. the
   no-clear control where it recalls.
2. **Process stays warm (no cold respawn):** a `pgrep -f codex-acp` watcher shows the shared process count is
   **unchanged across `clear`** (host-ACP); the post-clear turn pays **no cold start**. For `ContainerRwBackend`
   (separate gate), `docker ps` shows the OLD turn's container reaped and a fresh one for the next turn.
3. **Generation advances:** `session status C` shows `generation` incrementing on each `clear` (0→1→2); usage
   resets to empty (`used`/`size` null until the next turn repopulates).
4. **Force-clear a running turn (GENERATION-MONOTONICITY):** with a long turn in flight, `session clear C
   --force` cancels it + resets; the cancelled turn's late completion does **NOT** flip the new generation to a
   stale state or restore old usage — proven by `session status` showing the new generation Idle with
   fresh-zero usage, and a follow-up turn recalling NOTHING from before the clear. (Unit-gated for the precise
   race; live-gated for the end-to-end force path.)
5. **Require-Idle:** `session clear C` (no `--force`) while a turn is Running → `HandleBusy` (not a silent
   mid-turn reset).
6. **No regression:** Slice 0/1/2 DoD green (warm continue, reconcile, release, idle reap, usage telemetry,
   threshold warn) across a `clear`.

## Risks

- **The two-call window (release then configure):** between `release_session(old)` and `configure_session(new)`
  the handle is `Resetting` (claimed) so no turn can dispatch; a crash mid-window leaves the handle claimed →
  the re-acquire/revalidate path EXPIRES it (next checkout cold-remints). Gate: a fake backend whose
  `release_session` blocks → concurrent `checkout` is `HandleBusy`; `cancel`/`release` defer then EXPIRE.
- **GENERATION-MONOTONICITY ripple:** threading `generation` through `WarmTurn`/`WarmTurnGuard`/
  `finish_turn`/`record_usage` touches the producer sites (`server.rs` streaming + unary). Mis-threading would
  either drop a LIVE turn's completion (hang/leak) or fail to drop a stale one (corruption). Gate: a unit test
  where a gen-N `finish_turn` after a reset-to-N+1 is a no-op, AND a gen-N+1 `finish_turn` correctly idles.
- **Usage reset semantics:** clear zeroes `handle.usage` (fresh context). A late gen-N `record_usage` must NOT
  repopulate it (covered by the generation no-op). Don't reset usage on a NON-clear path.
- **`force` cancel ordering:** cancel the in-flight turn (`backend.cancel`) BEFORE entering `Resetting`, or the
  cancel races the release. Cancel → confirm the turn is torn down → then reset. The cancelled producer's
  `finish_turn`/`record_usage` are gen-guarded.
- **cwd reconstruction:** `fingerprint.cwd: Option<String>` → `SessionCwd::parse` for `configure_session`; a
  parse failure (shouldn't happen — it was valid at mint) maps to an internal error, not a panic.
- **ContainerRw per-session reap on clear:** `release_session(old)` reaps the old container; ensure the next
  turn's mint spins a fresh one (warm-mode). Gate `docker ps` old→0 + new→1.

## Testing approach

- **Unit (SessionManager, fake backend + fake clock):** `reset_session` on Idle → generation+1, new
  `ctx-{ctx}-g1` id, `release_session(old)` + `configure_session(new)` called in order, usage zeroed, state
  Idle, handle/lease/process kept; on `Running` without force → `HandleBusy`; with `force` → cancel then reset;
  `Resetting` blocks concurrent checkout (`HandleBusy`); a blocked-release + concurrent release/cancel →
  EXPIRE (`SessionExpired`) not a dirty handle; **generation guard:** `finish_turn(ctx, old_gen)` /
  `record_usage(ctx, old_gen, ..)` after a reset is a no-op while `(ctx, new_gen)` applies; `NotFound` on
  unknown ctx.
- **Integration (in-crate, mocked backend):** `session/clear` JSON-RPC returns `{cleared,generation}`;
  `session/clear` on a missing ctx → `SessionNotFound`; the producer threads the generation so a normal turn's
  `finish_turn` still idles (no-regression).
- **Live-gate (real serve + codex):** DoD 1–6 via `submit --context C` + `session clear C [--force]` +
  `session status C` + a `pgrep -f codex-acp` watcher (process count unchanged across clear). DoD-2 ContainerRw
  variant via `docker ps`. DoD-4's precise race is unit-gated; the live force path proves end-to-end.

## Constraints (carried)

codex gpt-5.5/high implementor (host, `run-workflow slice0-impl`; controller verifies + commits — the
`_dyld_start` flake); codex high-risk/final + Opus arch review; `max_attempts=3`; reviewers judge **intent,
not verbatim**. **Dual spec-review (codex xhigh + Opus) before planning** + **dual plan-review** +
**LIVE-GATED** before merge.

## Open decisions for the dual spec-review

- **D1 — reset via SessionManager composition (release_session + configure_session) vs. a dedicated atomic
  `AgentBackend::reset_session`.** Recommend composition (zero new code; `Resetting` serializes; fresh id makes
  a partial state harmless). Is the two-call window genuinely safe, or is atomicity worth a new trait method?
- **D2 — `force` scope.** Include cancel-then-reset in Slice 3 (so DoD-4 / GENERATION-MONOTONICITY is
  exercised end-to-end), or defer `force` and ship require-Idle only (making the generation guard purely
  defensive)? Recommend include — it's what makes the guard load-bearing and matches the architecture.
- **D3 — generation threading.** Thread `generation` through `WarmTurn`/`WarmTurnGuard`/`finish_turn`/
  `record_usage` (the proposed mechanism), or an alternative epoch check (e.g. a per-handle epoch the producer
  reads)? Recommend the explicit generation thread (typed, matches the captured-generation invariant).
- **D4 — `clear` vs `reset` naming on the wire.** `session/clear` (user verb) mapping `reset_session`
  (primitive). Confirm `clear` is the right public name (compact reuses the primitive, not the verb).
