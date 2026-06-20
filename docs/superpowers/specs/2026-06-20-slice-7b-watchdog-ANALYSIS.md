# Slice 7b — E9 watchdog: ARCHITECTURE ANALYSIS (controller pass)

> Elaborates the CONVERGED design-of-record (do NOT re-litigate the decomposition): the Slice-7 analysis
> `2026-06-20-slice-7-rich-acp-ANALYSIS.md` (DUAL-LENS CONVERGENCE, **D-E**) + architecture spec
> `2026-06-17-orchestration-architecture.md` (the WATCHDOG / UPDATE-MINIMAL invariants). Controller's analysis pass;
> a parallel codex-xhigh pass runs for dual-lens convergence. Builds on `slice-7a-rich-acp-shipped` (the journal
> liveness it needs); resume via the orchestration HANDOFF.

## What Slice 7b is (the converged D-E ruling)
The **E9 watchdog** — catch a HUNG turn without false-tripping a long-but-progressing one. Converged rulings (from
the Slice-7 analysis, do NOT reopen):
- **Per-active-NODE-turn, NOT a DB sweeper** — workflows run sibling nodes CONCURRENTLY (`executor.rs:412/455`
  `FuturesUnordered`); a task-level "last journal event" lets one chatty node MASK a hung sibling. The watchdog must
  watch each ACTIVE turn individually.
- **Two knobs, a NEW config namespace:** an **idle-timeout** (reset on each liveness event) + a **hard wall-clock**
  (from turn start) — NOT the warm idle-TTL or the ACP `cancel_grace` (different lifetimes).
- **Liveness = activity events, NOT strictly journal writes.** Any `ToolCall`/`ToolCallUpdate`/`Plan`/`Usage`/**text**
  chunk = liveness. (codex's S7a finding: text/usage are NOT journaled on the detached path → a strict "journal
  event = liveness" rule would false-trip a text-only turn; so the activity signal must be the agent EVENT stream.)
  The FN-1 `_dyld_start` case = a long `in_progress` tool_call emitting `ToolCallUpdate`s → those reset idle → NO
  false-trip. A no-chunk long model turn (silent "thinking") → the hard wall-clock backstop.
- **Cancel via the EXISTING path** — reuse the ACP cancel (`session/cancel` + arm `turn_kill` after grace,
  `acp_backend.rs:1906`); the watchdog does NOT invent a new kill. **Pending permission counts as activity; the
  `awaiting_permission` STATE is S9** (auto-approve is instant + off-loop today, so not needed for S7b's gate).
- **DoD:** a deliberately-hung turn (no events) is caught (idle or wall-clock); a long `in_progress` tool_call
  (steady `ToolCallUpdate`s) does NOT false-trip; the no-chunk long model turn hits the wall-clock backstop.

## Current state (ground-truth map — controller-verified)
- **The turn lifecycle is in `prompt_inner` (`acp_backend.rs:1907`):** acquires the per-session `turn_lock` (owned
  guard moved into the driver, held for the whole turn); builds the per-turn mpsc `(tx, rx)`; the driver task
  `block_task().await`s the `PromptResponse` while the SDK delivers chunks via the handler → `tx`; the returned
  `BackendStream` is the `unfold` over `rx` (S7a: the skip-rich loop). The driver `select!`s on the per-turn
  `turn_kill` Notify (`:315`, installed fresh per turn) — firing it unblocks a hung driver → the turn ends.
- **Cancel + grace (`acp_backend.rs:~1885-1935`):** `cancel(session)` sends `session/cancel` then, if the turn
  doesn't finish within `cancel_grace` (`AcpConfig.cancel_grace`, `:102`/`:1585`), fires `turn_kill`. This is the
  EXACT mechanism the watchdog reuses.
- **Activity is observable at the unfold:** every agent event (Text/Usage/Rich) arrives as a `TurnEvent` on `rx`
  and is `recv`'d in the unfold loop — the natural per-turn liveness tap. NO per-turn `last_activity` timestamp
  exists today (`SessionEntry` `:288-326` has none).
- **Config:** `AcpConfig` (`:95+`) holds `cancel_grace` etc.; a new `[watchdog]` block would be wired the same way
  the bridge config flows into `AcpConfig`.
- **S7a substrate (shipped):** rich events ARE journaled per detached node + flow through the unfold — so the
  liveness tap sees them. The journal is NOT the watchdog's input (the EVENT stream is); the journal remains the
  durable replay axis.

## Controller design (for dual-lens convergence)
**A per-turn watchdog task spawned in `prompt_inner`, observing the TurnEvent stream, firing the existing
cancel/`turn_kill`.** Applies to EVERY ACP prompt turn (each workflow node = one prompt turn → per-node-turn for
free; warm/unary turns get it too — a hung turn anywhere is worth catching; broader value, no per-path wiring).
- **Activity tap:** a per-turn `last_activity: Arc<AtomicI64>` (+ `first_event_seen: AtomicBool`) bumped in the
  unfold loop on EACH `rx.recv()` of a `TurnEvent` (any kind). The watchdog reads it.
- **The idle semantic (the false-trip fix, D-E):** the **idle-timeout counts only AFTER the first event** — a turn
  that has produced output then goes silent for `idle_timeout` = HUNG (fire); a turn that has produced NOTHING yet
  (a silent thinking model) has no idle clock → governed ONLY by the hard wall-clock. (`last_activity` = `None`
  until the first event; idle check skipped while `None`.) The FN-1 long tool_call resets `last_activity` on each
  `ToolCallUpdate` → never idles.
- **The watchdog task:** spawned in `prompt_inner` after the turn is set up; loops `sleep(check_interval)` →
  if `(first_event_seen && now - last_activity > idle_timeout) || (now - turn_start > wall_clock)` → fire the
  cancel path (`request_cancel` + `turn_kill` after grace, OR `turn_kill` directly for a hard timeout). Stops when
  the turn ends (a shared `done` flag / the turn_guard drop / a oneshot).
- **The cancel outcome:** a watchdog-fired cancel ends the turn like any cancel → the driver returns → `TurnEvent::
  Failed` (or `Done{cancelled}`) → the node fails. **Surface a DISTINCT reason** (e.g. `AgentTimedOut`/a watchdog
  stop_reason) so the workflow/transcript shows "timed out," not a generic failure (cf. P-4 `max_tokens` distinct).
- **Config:** a new `[watchdog]` block → `AcpConfig.watchdog: Option<WatchdogConfig { idle_timeout_secs,
  hard_wall_clock_secs }>` (absent = disabled; opt-in). Per-agent or global? (D-D.)

## Open decisions for the dual-lens pass (where the controller wants a second opinion)
- **D-A (placement / scope — pivotal):** the watchdog in `prompt_inner` applies to ALL ACP turns (detached + warm +
  unary). Is that right, or scope to DETACHED workflow nodes only (per the analysis's "per-node-turn" framing)? A
  warm interactive turn that's slow-but-alive — does the same idle/wall-clock fit, or do warm turns need different/
  no limits? Does cancelling a warm turn interact badly with the warm session lifecycle (the turn ends; the session
  stays warm — probably fine)?
- **D-B (the activity tap):** bump `last_activity` in the UNFOLD (per-turn `rx.recv`) vs in the HANDLER (`tx.send`,
  on the SDK loop — must stay non-blocking). The unfold is per-turn + off-loop → cleaner. Confirm the unfold sees
  EVERY event (text + usage + rich + the terminal) so liveness is complete.
- **D-C (the cancel mechanism + outcome):** reuse `cancel(session)` (graceful `session/cancel` → grace →
  `turn_kill`) vs fire `turn_kill` directly (hard). Should the watchdog do a graceful cancel first? And what
  TERMINAL does a watchdog-cancel produce — a distinct `AgentTimedOut`/stop_reason vs a generic `Failed`/`Canceled`?
  (The workflow + transcript should distinguish a timeout.)
- **D-D (config shape + defaults):** `[watchdog]` global vs per-agent; `idle_timeout_secs` + `hard_wall_clock_secs`;
  default DISABLED (opt-in) or sensible defaults? Interaction with `cancel_grace` (the watchdog's cancel uses the
  same grace).
- **D-E (the idle-only-after-first-event semantic):** is "no idle clock until the first event, wall-clock governs a
  silent turn" the right anti-false-trip rule? Any case it misses (a turn that emits one event then legitimately
  thinks for a long time → idle fires — is that acceptable, or should idle be generous)?
- **D-F (the watchdog task lifecycle):** how the per-turn watchdog task is cleanly TORN DOWN when the turn ends
  (no leaked tasks across thousands of turns) — a shared `Arc<AtomicBool> done` the unfold sets on terminal, a
  `tokio::select!` on a oneshot, or tie it to the `turn_guard` drop. Cancellation-safety of the watchdog vs the
  cancel path it fires (no double-cancel / no firing after the turn ended).

## DUAL-LENS CONVERGENCE (controller + codex-xhigh, both `sound-with-changes`)
codex corroborated the placement + the idle-after-first semantic, and CORRECTED four mechanics (controller-verified).
Binding outcome:

**Placement (CONFIRMED):** the watchdog lives in `AcpBackend::prompt_inner` (`acp_backend.rs:1907`) — the only layer
with the live ACP turn + `turn_kill`. Applies to ALL ACP turns when CONFIGURED (detached + warm + unary), opt-in.
An executor-level watchdog would duplicate the backend cancel plumbing AND miss warm/unary; warm lifecycle is NOT
corrupted (`WarmTurnGuard`/`finish_turn` returns the handle to Idle, `server.rs:530`/`session_manager.rs:498`) —
PROVIDED the timeout reports as an ERROR, not a user cancel.

**CORRECTION-1 (D-B — pivotal): bump activity at the HANDLER, not the unfold.** The unfold's `rx.recv` is gated by
DOWNSTREAM BACKPRESSURE (the local producer awaits `tx.send` to the SSE consumer `server.rs:1592`; the detached
drain awaits store writes `workflow_sink.rs:107`) → an actively-producing agent with a slow consumer would
false-timeout. The HANDLER (`acp_backend.rs:977/998`, on the SDK loop) is the REAL agent-activity point — bump a
per-turn `last_activity: Arc<AtomicI64>` there with a NON-BLOCKING atomic store (NONBLOCKING-ACP-HANDLERS holds).
Bump it for EVERY inbound `session/update` notification BEFORE `map_session_update` drops the unmodeled/non-text
ones (`acp_backend.rs:1814/1833`) — so even a dropped event counts as the agent being alive (a non-public
`Activity` signal, not just modeled `TurnEvent`s).

**CORRECTION-2 (D-C): a DISTINCT timeout outcome.** Set a per-turn `timed_out` flag BEFORE firing the cancel;
surface a NEW `BridgeError::AgentTimedOut` (→ A2A `Failed` via the `_ =>` default in the classifier). Do NOT reuse
`CancelTimeout` (→ A2A `Canceled`, `error.rs:120`) — else an agent that HONORS `session/cancel` returns
`Done{"cancelled"}` and the bridge reports a USER cancel, not a timeout (`translator.rs:188`, `executor.rs:276`).
Classify `AgentTimedOut` transient where the resilient layer needs it (`resilient.rs:18`).

**CORRECTION-3 (D-F): DRIVER-owned teardown, not unfold-owned.** The consumer can DROP the stream before the
terminal, so the unfold may never see it. The DRIVER has the all-exit cleanup point (unregisters the sender, clears
`turn_kill`, also fires on `done_sender.closed()`, `acp_backend.rs:2000`) — put the watchdog `done` signal THERE so
the per-turn task is torn down on EVERY exit (success/error/cancel/consumer-drop), no leak.

**CORRECTION-4: ARM after `ensure_session` + turn registration.** `request_cancel` can set a session-level cancel
LATCH before the agent session exists (cleared after mint) — a watchdog that arms too early could poison the NEXT
prompt. Arm only after the turn is fully set up.

**D-A:** `prompt_inner`, all ACP turns, opt-in (CONFIRMED). **D-E:** idle-only-after-first-event + hard wall-clock
(CONFIRMED); generous idle for "emit once then think" agents; a silent model turn → wall-clock only. **D-D:**
PER-AGENT config (`AgentEntry`→`AcpConfig`, `config.rs:997`/`main.rs:252`) + an optional global default; absent =
DISABLED. **Thread `[watchdog]` through `ContainerRwConfig`** (`bridge-container/lib.rs:249`) or container/
`implement` runs silently miss E9. API (`kind="api"`) backend is OUT (this is ACP E9).

**Clock:** monotonic `std::time::Instant`/elapsed — NOT epoch ms (the watchdog is runtime-local; core's injected-
timestamp rule doesn't apply in `bridge-acp`).

**Blast-radius (CONFIRMED hazard, accept + document):** the cancel last-resort `escalate_terminate`
(`acp_backend.rs:1607`) SIGTERM/SIGKILLs the WHOLE shared agent process (all multiplexed sessions) — a hung agent
that IGNORES `session/cancel` makes a watchdog timeout fail SIBLING turns on that backend, and the killed process
isn't auto-replaced unless the slot retires (`registry.rs:305`). The watchdog does GRACEFUL `session/cancel` FIRST
(per-turn, no blast radius for a well-behaved agent) and inherits the EXISTING cancel's last-resort (it adds no NEW
escalation). Document the tradeoff; do not over-engineer per-turn process isolation in S7b.

**Cut/Defer:** CUT the DB/journal sweeper, the executor-level watchdog, and a `turn_kill`-only (no-graceful) timeout.
Surface timeout as `Failed` + the `AgentTimedOut` reason — NO new A2A `TimedOut` task state. Do NOT defer the
distinct reason (operationally ambiguous without it). DEFER: the API-backend watchdog (S-later), full
pending-permission activity/UI (S9 — but permission requests bypass `TurnEvent` `acp_backend.rs:1021`, so leave the
activity seam ready), and a user-configurable `check_interval` (use a small internal deadline loop).

## DoD / gate (roadmap)
A detached run with (a) a deliberately-hung node (an agent that accepts the prompt then emits NOTHING) is cancelled
by the wall-clock (and, if it emitted then stalled, by idle); (b) a long `in_progress` tool_call (steady
`ToolCallUpdate`s) runs to completion — NOT false-tripped; (c) the watchdog task does not leak. Live-gate vs real
codex (a `sleep`-style hung tool / a long-running tool that keeps emitting updates).
