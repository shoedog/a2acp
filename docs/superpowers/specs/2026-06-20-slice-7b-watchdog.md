# Slice 7b — E9 watchdog — Spec

> **Status:** v2 (dual spec-review folded — codex-xhigh + Opus, both `fix-then-plan`; FIX-1..12 below are BINDING
> and SUPERSEDE contradicting body text). The architecture holds; the mechanism was reshaped around the DRIVER
> owning the terminal (a `select!` arm) + the route-value carrying the watch. Next: plan → dual plan-review.
> **Companion analysis (binding rulings):** `docs/superpowers/specs/2026-06-20-slice-7b-watchdog-ANALYSIS.md`
> (DUAL-LENS CONVERGENCE). **Design-of-record:** `2026-06-17-orchestration-architecture.md` (the WATCHDOG /
> UPDATE-MINIMAL invariants). **Roadmap:** slicing row 7 (S7b half). Builds on `slice-7a-rich-acp` (merged `8e1e5c6`).

## Goal
Catch a HUNG agent turn without false-tripping a long-but-progressing one. A per-turn **E9 watchdog** in
`AcpBackend::prompt_inner` observes per-turn ACTIVITY and fires the EXISTING graceful cancel when a turn exceeds an
**idle-timeout** (silence after producing output) or a **hard wall-clock** (total turn time). The cancelled turn
surfaces a DISTINCT `AgentTimedOut` outcome (→ A2A `Failed`), so a workflow/transcript can tell a timeout from a
user cancel. Opt-in per agent; applies to all ACP turns (detached + warm + unary).

## v2 — dual spec-review fixes folded (BINDING; SUPERSEDES contradicting body text)
Dual spec-reviewed (codex-xhigh + Opus, BOTH `fix-then-plan`; the ARCHITECTURE holds — per-turn watchdog, handler
activity, idle-after-first, distinct outcome — but the MECHANISM is reshaped around the DRIVER owning the terminal).
Read FIX-1..12 FIRST.
- **FIX-1 (BLOCKER — codex #1/#2/#4, Opus #4/#9) — the DRIVER owns the terminal; the watchdog only SIGNALS.** The
  watchdog task CANNOT call `self.request_cancel` (spawned tasks are `'static`; `prompt_inner` has `&self`), and
  `request_cancel` alone does NOT escalate (the grace-watcher/`turn_kill` lives in `cancel()`). → add a per-turn
  `watchdog_fired: Arc<tokio::sync::Notify>`. The watchdog task is `'static` (captures `watchdog_fired` +
  `Arc<TurnWatch>` + the two `Duration`s + a `done` oneshot — NO `&self`); on timeout it `watchdog_fired.notify_one()`
  and exits. The DRIVER's EXISTING `tokio::select!` (`acp_backend.rs:1971`) gains a `_ = watchdog_fired.notified()
  => { … }` arm that runs the SAME bounded cancel the `done_sender.closed()` arm already does (`CancelNotification`
  → inner `select!{ prompt_fut | kill | sleep(grace)→escalate_terminate }`) and sets a DRIVER-LOCAL `timed_out =
  true`, yielding `Err(())`. The terminal `match outcome` (`:2010`) checks that LOCAL flag → emits
  `TurnEvent::Failed(BridgeError::AgentTimedOut)`. **No race:** the `select!` atomically picks the winner — if
  `prompt_fut` completes first the watchdog arm never runs, so a naturally-completing turn is NEVER mislabeled (no
  shared `timed_out` atomic in the terminal decision).
- **FIX-2 (BLOCKER — Opus #2/#3, codex #3/#10) — the routing-registry VALUE carries the watch + `turn_start`.** The
  registry is `Arc<StdMutex<HashMap<AgentSessionId, UpdateSender>>>` (`acp_backend.rs:211`). → change the value to
  `struct TurnRoute { tx: UpdateSender, watch: Option<Arc<TurnWatch>> }` (watch `None` when the watchdog is disabled
  → byte-identical) where `struct TurnWatch { turn_start: std::time::Instant, last_activity_ms: AtomicU64 }`. Update
  the driver INSERT (`:1934`), REMOVE (`:2003`), and the handler GET (`:999`). The key is the **`AgentSessionId`**
  (`notif.session_id` / the `agent_id` the driver inserts) — say `agent_session_id` throughout, NOT bridge
  `SessionId`.
- **FIX-3 (BLOCKER — Opus #1, codex #5/#8) — bump activity UNCONDITIONALLY at the TOP of the handler.** Today the
  handler only locks when `te.is_some()` (`:985-1003`) → unmodeled events (thought chunks, dropped variants) never
  bump. → put the bump in a SEPARATE short lock scope at the TOP of the handler closure, BEFORE `let te =
  map_session_update…`: `if let Ok(map)=updates.lock() { if let Some(r)=map.get(&notif.session_id) { if let
  Some(w)=&r.watch { w.last_activity_ms.store(w.turn_start.elapsed().as_millis() as u64 + 1, Relaxed) } } }`. ALSO
  bump on `RequestPermissionRequest` (`:1021`) — the design-of-record requires pending permission count as activity
  (do the BUMP now; defer only the `awaiting_permission` STATE to S9). Weaken the KEYSTONE wording: "short sync
  `StdMutex`, no `.await`" (the registry is a `StdMutex`; the bump holds it briefly).
- **FIX-4 (MAJOR — codex #6, Opus #6) — `u64` cast + a safe sentinel.** `Instant::elapsed().as_millis()` is `u128`
  → `as u64`. `0` is UNSAFE (a first event at elapsed 0ms) → store `elapsed_ms + 1` (`0` = no event yet). Idle check:
  `let la = last_activity_ms.load(Relaxed); if la != 0 { … }`. Import `AtomicU64` (only `AtomicBool` is imported
  today, `:13`).
- **FIX-5 (MAJOR — codex #7) — saturating duration math.** The handler can store a NEWER `last_activity` between the
  watchdog computing `elapsed` and loading `la` → `elapsed_ms - la` underflows/panics. Use `saturating_sub` (treat
  future activity as idle 0).
- **FIX-6 (MAJOR — Opus #5) — `AgentTimedOut` is FATAL, not transient.** Add `BridgeError::AgentTimedOut`; `to_state`
  leaves it on `_ => SetState(Failed)` (NOT `Canceled` like `CancelTimeout` `error.rs:120`). `classify_death`
  (`resilient.rs:18`) leaves it on `_ => Fatal` — do NOT add it to the transient arm (else `ResilientWarm` RETRIES a
  hung agent → it re-hangs + burns `max_respawns`). DELETE the data-model's "transient where appropriate." NB:
  `BridgeError` is matched EXHAUSTIVELY in some helpers/tests → update those arms (not just a `_` fall-through).
- **FIX-7 (MAJOR — both #8/#9) — the FULL config ripple; per-agent ONLY (CUT the global default).** Per-agent
  `[agents.watchdog]` mirrors `[agents.sandbox]`; **CUT the top-level `[watchdog]` global default** (no precedent /
  merge machinery exists — net-new). FIVE+ edit sites: `WatchdogToml` parse sub-table (`config.rs`) → domain
  `AgentEntry` field (`bridge-core/domain.rs:115`) → `into_snapshot` conversion (`config.rs`) → `AcpConfig.watchdog`
  build (`main.rs:252`, struct-update `..default()`) → `WatchdogConfig` + `Default` on `AcpConfig` (`acp_backend.rs`).
  PLUS the container path: a `ContainerRwConfig` field (`bridge-container/lib.rs:41/249`) + the build site
  (`main.rs:441`) + forward into the inner `AcpConfig`. Validate `idle_timeout_secs`/`hard_wall_clock_secs` `> 0`.
  API (`kind="api"`) ignores it.
- **FIX-8 (MINOR — Opus #10) — teardown via a `oneshot`.** The driver, at its all-exit cleanup (`:2002`, after
  `map.remove`), drops a `oneshot::Sender` → the watchdog `select!`s `sleep(deadline)` vs `done_rx` and exits on
  EITHER (no `AtomicBool`+`Notify` lost-wakeup). The `TurnRoute` remove at `:2003` drops the `watch` (single remove).
- **FIX-9 (MINOR — Opus #11) — sleep to the NEXT DEADLINE, no fixed tick.** Each loop: `deadline = min(turn_start +
  hard_wall_clock, last_activity + idle_timeout if seen)`; `sleep_until(deadline)` → re-check (precise + cheap, no
  wakeup storm across concurrent `FuturesUnordered` turns). On a spurious early wake (activity advanced), re-derive.
- **FIX-10 (MINOR — Opus #12) — blast-radius DEPLOYMENT note.** The watchdog makes `escalate_terminate`
  (`:1607`, SIGKILLs the WHOLE shared process) fire AUTOMATICALLY on a hung turn that ignores `session/cancel` past
  grace → siblings on a multiplexed local-process agent die + the process isn't auto-replaced (`registry.rs:305`).
  Doc guidance: enable primarily on container/per-turn-isolated agents; shared-process backends inherit sibling-kill.
- **FIX-11 (NIT — Opus #13) — DoD adds an UNMODELED-event-only case** (an agent emitting ONLY thought chunks /
  unmodeled updates is NOT false-tripped) to actually exercise the "dropped event still counts as alive" keystone
  (FIX-3) — the existing ToolCallUpdate case wouldn't catch a regression there (it's modeled).
- **FIX-12 (NIT — Opus #14) — `ContainerRwBackend` composes `AcpBackend` per turn** (`bridge-container/lib.rs:249`)
  → it gets E9 by COMPOSITION once the config forwards (FIX-7); the watchdog lives ONLY in `prompt_inner`, no second
  implementation.

## KEYSTONE constraints (do not violate)
- **NON-BLOCKING activity tap at the HANDLER.** Bump the per-turn `last_activity` in the SDK notification handler
  (`acp_backend.rs:977`), with a plain atomic store — NEVER `.await`/block (NONBLOCKING-ACP-HANDLERS). Do it for
  EVERY inbound `session/update` BEFORE `map_session_update` drops the unmodeled/non-text ones (so a dropped event
  still counts as the agent being alive). NOT the unfold (its `rx.recv` is gated by downstream backpressure →
  false-timeout).
- **DISTINCT timeout outcome.** A new `BridgeError::AgentTimedOut` mapped to A2A `Failed` (the classifier `_ =>`
  default, NOT `CancelTimeout`→`Canceled`). The watchdog sets a per-turn `timed_out` flag BEFORE firing cancel, and
  the driver emits the timeout terminal — so an agent that honors `session/cancel` (returns `Done{"cancelled"}`) is
  reported as a timeout, not a user cancel.
- **DRIVER-owned teardown.** The per-turn watchdog task is stopped at the DRIVER's all-exit cleanup point
  (`acp_backend.rs:2000`) — fires on success/error/cancel/consumer-drop — so no watchdog task leaks across turns.
  (The unfold can't own teardown: the consumer can drop the stream before the terminal.)
- **ARM after `ensure_session` + sender registration.** `request_cancel` sets a session-level latch that, if armed
  before the agent session exists, poisons the NEXT prompt — so the watchdog arms only once the turn is fully set
  up.
- **GRACEFUL cancel first; no NEW escalation.** The watchdog reuses the EXISTING cancel path (`request_cancel`
  graceful `session/cancel` → `cancel_grace` → `turn_kill`/`escalate_terminate`). It adds no new kill. **Documented
  blast-radius:** `escalate_terminate` SIGKILLs the WHOLE shared agent process (`acp_backend.rs:1607`) — a hung
  agent that IGNORES cancel makes a watchdog timeout fail sibling turns on that multiplexed backend; graceful-first
  minimizes it; S7b does NOT add per-turn process isolation.
- **Monotonic `Instant`.** All watchdog timing is runtime-local `std::time::Instant`/`Duration` — NOT epoch ms.
- **Opt-in.** Absent `[watchdog]` config = DISABLED → `prompt_inner` is byte-identical to today (no watchdog task
  spawned).

## Scope
**IN:**
- `WatchdogConfig { idle_timeout_secs, hard_wall_clock_secs }` on `AcpConfig` (per-agent) + an optional global
  default; threaded through `ContainerRwConfig` so container/`implement` runs get E9.
- The per-turn activity tap (handler atomic) + the watchdog task (spawned in `prompt_inner`, driver-torn-down) + the
  idle-after-first + wall-clock logic + the graceful-cancel-with-`timed_out`.
- `BridgeError::AgentTimedOut` → A2A `Failed`; the driver emits it on a watchdog-fired turn.
- DoD: a hung turn is cancelled (idle or wall-clock); a long `in_progress` tool_call (steady `ToolCallUpdate`s) is
  NOT false-tripped; no watchdog task leaks.

**OUT (deferred):**
- The `kind="api"` (non-process) backend watchdog — this is ACP E9 (the API adapter would need its own timeout).
- Full pending-permission ACTIVITY/state → **S9** (permission requests bypass `TurnEvent` — `acp_backend.rs:1021`;
  leave the activity seam ready, but don't build the `awaiting_permission` state).
- A new A2A `TimedOut` task state (surface as `Failed` + the `AgentTimedOut` reason).
- A user-configurable `check_interval` (use a small fixed internal deadline loop).

## Data model (per FIX-1/2/6; crate home corrected in plan PFIX-A/C)
- **`crates/bridge-core/src/domain.rs`** (next to `SandboxConfig`) — `pub struct WatchdogConfig { pub idle_timeout:
  Duration, pub hard_wall_clock: Duration }` (both `> 0`, validated). **`crates/bridge-acp/src/acp_backend.rs`** —
  `AcpConfig` gains `pub watchdog: Option<bridge_core::domain::WatchdogConfig>` (None = disabled, no task spawned).
  (WatchdogConfig MUST live in bridge-core because `AgentEntry` carries it and bridge-core can't depend on
  bridge-acp.)
- **The routing-registry VALUE carries the watch (FIX-2):** the `UpdateRegistry` value changes from `UpdateSender`
  to
  ```rust
  struct TurnRoute { tx: UpdateSender, watch: Option<Arc<TurnWatch>> }   // watch None = disabled (byte-identical)
  struct TurnWatch { turn_start: std::time::Instant, last_activity_ms: AtomicU64 }  // store elapsed_ms+1; 0 = none
  ```
  Keyed by `AgentSessionId` (`notif.session_id`). The handler bumps `route.watch.last_activity_ms` (FIX-3); the
  driver + the watchdog task share the `Arc<TurnWatch>`.
- **The terminal decision is DRIVER-LOCAL, not a shared atomic (FIX-1):** a per-turn `watchdog_fired: Arc<Notify>`
  (the watchdog signals it; the driver `select!`s on it). No `timed_out` atomic in the terminal path — the driver's
  `select!` arm IS the decision.
- **`crates/bridge-core/src/error.rs`** — add `BridgeError::AgentTimedOut`; `disposition()` (NOT `to_state` — plan
  PFIX-C) → `SetState(Failed)` via the `_` default (NOT `Canceled`); `classify_death` (`resilient.rs`) → `Fatal`
  (the `_` default — NOT transient, FIX-6); the ONE exhaustive match to update is `table_key` (`resilient.rs:154`)
  + the exhaustiveness Vec (`:183`).

## The mechanism (`prompt_inner` + the DRIVER) — per FIX-1/2/3/8/9
1. **Setup (only when `config.watchdog.is_some()`, AFTER `ensure_session` + the `TurnRoute` insert):** build
   `Arc<TurnWatch>{ turn_start: Instant::now(), last_activity_ms: 0 }`, store it in the turn's `TurnRoute.watch`;
   build `watchdog_fired: Arc<Notify>` + a `done` oneshot.
2. **Handler activity bump (FIX-3, `acp_backend.rs:977`):** at the TOP of the handler closure (a short
   `updates.lock()` scope, BEFORE `map_session_update`), `route.watch.last_activity_ms.store(turn_start.elapsed()
   .as_millis() as u64 + 1, Relaxed)`. Same bump on `RequestPermissionRequest` (`:1021`). Short sync lock, no
   `.await`.
3. **The watchdog task (`'static`, FIX-1/9):** captures `Arc<TurnWatch>` + `watchdog_fired` + the two `Duration`s +
   the `done` receiver — NO `&self`. Loop: derive `deadline = min(turn_start + hard_wall_clock,  (la!=0 ?
   la_instant + idle_timeout : ∞))`; `tokio::select! { _ = sleep_until(deadline) => …, _ = &mut done_rx => return }`.
   On wake: re-load `la`; if a wall-clock OR (seen && idle) deadline is truly passed → `watchdog_fired.notify_one()`;
   return. Else (activity advanced) re-derive + loop. Use `saturating_sub` for the idle math (FIX-5).
4. **The DRIVER select! arm (FIX-1, `acp_backend.rs:1971`):** add `_ = watchdog_fired.notified() => { <the SAME
   bounded cancel the done_sender.closed() arm runs: send CancelNotification; inner select!{ prompt_fut | kill |
   sleep(grace)→escalate_terminate }>; timed_out_local = true; Err(()) }`. Because the driver's outer `select!`
   atomically picks ONE arm, a `prompt_fut` completion can't be retro-labeled.
5. **Driver teardown (FIX-8):** at the all-exit cleanup (`:2002`) the driver drops the `oneshot::Sender` (→ the
   watchdog's `done_rx` resolves → it exits) and `map.remove(&agent_session_id)` (drops the `TurnRoute` incl. the
   `watch`). Runs on EVERY exit (Ok/Err/kill/consumer-drop).
6. **The terminal outcome (FIX-1/6):** the terminal `match outcome` (`:2010`) uses the driver-LOCAL `timed_out_local`
   (set only in the watchdog arm) → `TurnEvent::Failed(BridgeError::AgentTimedOut)` (NOT the generic
   `agent_crashed`, and NOT the `Done{"cancelled"}` an honored cancel produces) → the stream yields `Err(AgentTimed
   Out)` → A2A/executor mark the node `Failed` with a distinct timeout reason.

## Config wiring (per FIX-7 — per-agent ONLY; the full ripple)
- TOML: `[[agents]]` gains an optional `[agents.watchdog] idle_timeout_secs = N, hard_wall_clock_secs = M` (mirrors
  `[agents.sandbox]`). **NO top-level `[watchdog]` global default** (CUT — no merge precedent). Durations validated
  `> 0`.
- The 5+ ripple sites: `WatchdogToml` parse sub-table (`config.rs`) → domain `AgentEntry` field
  (`bridge-core/domain.rs:115`) → `into_snapshot` (`config.rs`) → `AcpConfig.watchdog` build (`main.rs:252`,
  struct-update) → `WatchdogConfig` + `Default` (`acp_backend.rs`).
- The CONTAINER path: a `ContainerRwConfig` field (`bridge-container/lib.rs:41/249`) + the build site
  (`main.rs:441`) + forward into the inner `AcpConfig` — so `implement`/container runs get E9 by COMPOSITION
  (`ContainerRwBackend` builds an `AcpBackend` per turn; FIX-12). API (`kind="api"`) ignores it.

## DoD / gate
1. **Hung turn caught:** a fake/real agent that accepts the prompt then emits NOTHING is cancelled by the
   `hard_wall_clock`; one that emits then goes silent is cancelled by the `idle_timeout`; the turn surfaces
   `AgentTimedOut` → A2A `Failed` (NOT `Canceled`).
2. **No false-trip:** a long `in_progress` tool_call emitting steady `ToolCallUpdate`s (each bumping `last_activity`)
   runs to completion — never cancelled. **AND (FIX-11)** a turn emitting ONLY UNMODELED updates (e.g. agent thought
   chunks the mapper drops) is NOT false-tripped — exercising "a dropped event still counts as alive" (FIX-3).
3. **No leak / opt-in:** the watchdog task is torn down on every turn exit (success/error/cancel/consumer-drop); with
   no `[watchdog]` config, `prompt_inner` spawns no watchdog (byte-identical to today). Existing acp turn/cancel
   tests pass.
4. **Gate:** `cargo fmt --all` + `cargo clippy --all-targets` clean; full workspace tests green (ci.yml floors);
   live-gate vs real codex (a hung tool vs a long-but-emitting tool).
