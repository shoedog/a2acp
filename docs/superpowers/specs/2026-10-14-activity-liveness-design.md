# Activity liveness: silence cutoff, opt-in hard cap, client heartbeat (design)

**Status:** revision 3. Sol/xhigh round 1/2 REJECTED 5 WRONG / 5 SMELL; round 2/2 REJECTED 5 WRONG / 4 SMELL.
At the cap the controller classified the workflow hard-cap opt-in (now **I2b**) as **open-class** (each round
found new consumers of the work cutoff that break when it is unarmed) and **parked it** pending a complete
consumer enumeration; the remaining findings are closed and bounded and are folded here for I1, I2a and I3.
Ledger: `../reviews/2026-10-14-activity-liveness-spec-review.md`. Issues #47 and #24.
**Base:** `origin/main` `5e3fec9367300bc12c37840f9d4ccaeebad2b04f`.
**Owner decisions:** 2026-10-13 (items 1-4 below) and 2026-10-14 (the hard-cap default and the rollback
limit). The two small sibling fixes (unary turn metrics, fenced-rename trace class) are a separate task and
are out of scope here.

## 1. Problem

A turn whose agent goes silent has no bound short of the workflow's 2-hour wall clock, and served unary turns
have no bound at all (#47: a served Codex submit sat silent for an hour). Workflows cannot use the per-agent
watchdog: admission refuses it whenever automatic deadlines are on (`crates/bridge-workflow/src/admission.rs`
:320-328), and the automatic policy has no silence cutoff (`SilenceCutoffV1::None`,
`crates/bridge-core/src/execution_policy.rs` :113). Meanwhile the 2-hour cap cuts long legitimate runs, and a
waiting CLI client sees nothing until the turn ends.

## 2. Owner decisions (fixed)

1. **Activity** is any ACP `session/update` (message, thought, plan, tool call or update, usage, and any other
   variant including metadata updates) or a permission request. While a **tool call is open** or a
   **permission request is pending**, the silence clock pauses, so a long `cargo test` never trips it.
2. **Silence cutoff = 30 minutes**, from measured gaps in real runs: longest legitimate gap 15.4 min (a
   compaction), Codex p99.9 18.1 min, Claude max 14.8 min. A usage update whose totals did not change is not
   activity (Codex re-sends usage during provider retries). Claude thought chunks with empty text are activity.
   A hung tool call is not caught by the silence cutoff; that is accepted, and it is handled by item 3 or by
   the client (item 4).
3. **The 2-hour hard cap becomes an opt-in backstop for watched runs and stays on by default for unwatched
   runs** (owner, 2026-10-14). "Watched" means an interactive CLI client is waiting and printing the
   heartbeat of item 4. Any run can explicitly turn the cap on or off. **Controller refinement (round 1):** a
   run counts as watched only when its lifetime is bound to that client, so the client leaving ends the run:
   local `run-workflow` (in-process) and served unary `message/send` whose dropped request cancels the turn
   (§4.5). Served or detached workflows (`run-workflow --serve`, detached `submit`, batch, boot-resumed runs)
   outlive their client, so they are always unwatched for the default; their client may still opt out
   explicitly. This avoids a watcher-lease mechanism.
4. **The client sees activity and decides:** `submit` and `run-workflow` (local and `--serve`) print a
   periodic stderr heartbeat. Served clients also have `task watch` and `task cancel`.
5. **Rollback limit accepted** (owner, 2026-10-14): binaries older than this change cannot decode the run-policy
   records new automatic workflow runs write. Document it in the operator's SERVICE.md; no tolerant-reader
   pre-release.

## 3. Amendment to the R2f owner design

`docs/superpowers/specs/2026-07-20-r2f-owner-design.md` §2 invariant 1 ("No workflow mode is unbounded",
:100) and :197-200 ("not killed merely for silence ... the absolute deadline always bounds them eventually")
are amended as follows, by the owner's 2026-10-14 decision:

- A **watched** run (item 3) has no absolute work cutoff unless it opts in. Its bounds are the activity
  silence cutoff (30 min, paused during open tool calls and pending permissions) and the client, who sees the
  heartbeat and can cancel. A hung tool in a watched run is therefore bounded only by the client.
- An **unwatched** run keeps the absolute work cutoff (default 2 h) and additionally gets the silence cutoff.
- Turns are now killed for silence (item 2). The no-progress *warning* cadence is unchanged.
- Everything else in R2f (queue admission, cancellation and cleanup bounds, fixed grace, impossibility
  proofs) is unchanged.

This PR edits the R2f document to point at this amendment.

## 4. Design

### 4.1 One enforcement point: the turn

Every turn reaches `AcpBackend::prompt`, and every caller stages a `TurnMeta` through `configure_turn`
(`acp_backend.rs` :7869; callers `server.rs` :767/:2680/:3865, `coordinator.rs` :1251, `executor.rs`
:2491/:3079/:3995, `main.rs` :2743). The cutoff is enforced once, in `TurnWatch`, with the per-turn request
carried on `TurnMeta` (`crates/bridge-core/src/permission.rs` :11).

- A new pure tracker in `crates/bridge-core/src/turn_liveness.rs` holds the activity state. It reads no clock;
  callers pass monotonic elapsed milliseconds (the style of `no_progress_warning.rs`).
- `TurnWatch` becomes always armed and wraps the tracker. It switches from `std::time::Instant` to
  `tokio::time::Instant` so paused-time tests can drive it (production behaviour is identical).
- The legacy `[agents.watchdog]` keeps its exact semantics and failure code, as a separate deadline in the
  same timer task. Its admission refusal under automatic deadlines is unchanged.

### 4.2 Semantics

**Activity classification** happens on the typed `SessionUpdate`, replacing the unconditional bump
(`acp_backend.rs` :4341-4344):
- every variant counts, including `AgentThoughtChunk` with empty or non-text content (classify by variant,
  not by `thought_delta_len`) and unmodeled, terminal-evidence and prefix-control frames;
- **except** a `UsageUpdate` whose totals key `(used, size, Option<(cost.amount.to_bits(), cost.currency)>)`
  (cost is optional in the ACP mapping, `acp_backend.rs` :6902) equals the previous usage update's key in the
  same turn: repeated `None` cost is unchanged; `None`→`Some` and a currency-only change are changes. The first usage update counts. Compare with the previous
  update, not a high-water mark, so a drop after compaction counts. `TurnProgress.usage` is unchanged. Final
  usage arrives with the prompt response, which ends the turn.
- a permission request's arrival and its resolution both count.

**Open tool calls**, kept in the tracker as a map keyed by the SHA-256 digest of the **raw** `toolCallId` (not
the capped id from `map_session_update_rich`, which can collide; the digest bounds memory per entry whatever
the id's length), value `{id preview ≤ 64 bytes, title ≤ 128 bytes (UTF-8-safe truncation), kind, opened_ms}`:
- `ToolCall` or `ToolCallUpdate` with status `pending` or `in_progress` opens the id (or refreshes it);
  `completed` or `failed` closes it; any other status, or no status, changes nothing. An update for an
  unknown id with `pending`/`in_progress` opens it (title/kind from the update, else `"?"`); an unknown id
  with `completed`/`failed` is a no-op. All of these are activity.
- At most 4096 simultaneously open entries (≈ 0.9 MiB worst case). Closing an entry frees its slot, so many
  tools opened and closed over a turn never overflow. Only a 4097th tool open **at the same time** sets a
  sticky `open_overflowed` for the rest of the turn (trace event `acp.liveness_tool_set_overflow`), which
  counts as paused (fails open). This needs 4097 concurrently open tool calls, which no adapter produces; an
  unwatched turn still has its work cutoff.
- `TurnProgress.tool_states` stays as it is.

**Pending permissions:** a counter (not a flag: two may be pending at once) incremented on arrival (`acp_backend.rs` :4499) and decremented by an RAII
guard moved into the permission task around `resolve_permission_outcome`, so it runs on every path
including drop.

**Clock:** `paused = open non-empty || open_overflowed || pending_permissions > 0`.
`anchor = max(last_activity_ms, last_unpause_ms)`; silence is due at `anchor + silence_ms` while not
paused. The timer sleeps until the earliest of silence-due, turn-work-due and the legacy deadlines; it is
woken on turn end or by a `Notify` raised on paused→unpaused transitions; it re-checks under the tracker
lock before firing. If two deadlines are due at the same instant, the cause is chosen by a fixed priority:
legacy watchdog, then work cutoff, then silence.

**Failures** (new `PromptDriverFailure` variants, mapped next to the watchdog at `acp_backend.rs` :7711).
Both are class `Timeout`, `FailureDisposition::Fatal`, `prompt_may_have_been_accepted = true` (no retry, no
fallback):
- silence → `acp.prompt.silence_cutoff`, cause "ACP prompt had no activity for {N}s with no tool call or
  permission open";
- turn work cutoff → `acp.prompt.work_cutoff`. It fires regardless of open tools: it is the backstop.

The cancel/settle/escalate path is the watchdog's (`settle_prompt_after_cancel`, then `escalate_terminate`).
In a workflow node, a Timeout-class `AgentFailure` already becomes node primary `TimedOut` with the cause code
(`executor.rs` :6026-6031); no terminal schema change.

### 4.3 Resolution of the per-turn values

`TurnMeta.liveness: TurnLivenessRequestV1 { silence: Inherit|Off|Ms(u64), work_cutoff: Inherit|Off|Ms(u64),
client_watching: bool, observer_key: Option<String> }`, default `Inherit, Inherit, false, None`.

- **Silence:** request > agent `[agents.liveness] silence_cutoff_secs` (0 = off) > 30 min.
- **Turn work cutoff:** request > agent `[agents.liveness] work_cutoff_secs` (0 = off) > the watched default:
  off when `client_watching`, 2 h (`DEFAULT_WORK_CUTOFF_MS`) when not.
- **Workflow nodes** carry `work_cutoff: Off` on the turn: the workflow-wide cutoff (§4.4) is their backstop.
  Their silence value comes from the frozen workflow controls (§4.4). **Until I2 lands, I1 writes
  `silence: Off` and `work_cutoff: Off` at all three workflow turn sites**, so I1 alone changes no workflow
  behaviour; I2 then maps the frozen controls.
- Every duration from any source (agent config, request metadata, workflow config, invocation/CLI) goes
  through ONE checked parser: whole seconds, ≤ 30 days (the watchdog cap at `config.rs` :2196), checked
  seconds→milliseconds conversion; anything else is a typed refusal (`InvalidRequest` for requests, config
  error for config). Tests: 30 days, 30 days + 1 s, and `u64::MAX`.
- `container_rw` agents: the inner `AcpConfig` is built in `crates/bridge-container/src/lib.rs` (~:847), not at
  `main.rs` :1163, which builds `ContainerRwConfig`. Add the resolved agent liveness config and the shared
  board to `ContainerRwConfig` (~:201), populate them in `container_rw_cfg_from_entry`, and forward both to
  the inner `AcpConfig`.
- Per request (served): metadata `a2a-bridge.silence_cutoff_secs`, `a2a-bridge.work_cutoff_secs`
  (`"off"`, `"0"` or N) and `a2a-bridge.client_watching` (`"true"`), parsed in
  `crates/bridge-coordinator/src/params.rs` :265 into a separate field, **not** `AgentOverride` (which feeds
  `effective_config` and identities). Invalid values are `InvalidRequest`.
- `AgentEntry.liveness` is encoded into the provider-effect digest (`freeze_effect`, `execution_policy.rs`
  :2495) as a trailing field **only when set**, following the `rename_new_sessions` precedent, so agents
  without it keep their digests.

### 4.4 Workflow policy

**Silence:** extend `SilenceCutoffV1 { None, AgentDefault, Activity { silence_ms } }`. `None` keeps its
meaning (no silence cutoff: legacy persisted runs, manual activation, explicit off). `liveness_profile_v1()`
is unchanged; `resolve_execution_policy_with_readiness_v1` (:1541) sets `AgentDefault` (or the explicit
workflow/invocation value) **only under `AutomaticR2f1b`**, so manual controls, the manual LITERAL
(`crates/bridge-core/tests/r2f1a_execution_policy.rs` :61), `MANUAL_WORKLOAD_FINGERPRINT_GOLDEN`
(`crates/bridge-workflow/tests/r2f1b_workload_identity.rs` :43) and the manual shape golden (`graph.rs` :568)
do not move (precedent: `graph.rs` :543). The executor maps `None` → Off, `AgentDefault` → Inherit,
`Activity` → Ms onto `TurnMeta.liveness.silence` at its three turn sites.

**Silence overrides (I2a):** tri-state everywhere: absent = inherit, `Off`, or a duration. Define
`LivenessOverrideV1 { Off, Ms(u64) }` and carry `Option<LivenessOverrideV1>` for silence in `[[workflows]]`
TOML (`silence_cutoff = "off" | <secs>`; add the field explicitly: `WorkflowToml` is `deny_unknown_fields`), in
`ExecutionPolicyInvocationV1`, and in `run-workflow --silence-cutoff <secs|off>` (replacing `::default()` at
`main.rs` :4624). `off` never collapses into absence. Precedence: invocation > workflow > `AgentDefault`. The
resolved value is frozen (`SilenceCutoffV1::None` for off, `Activity` for a duration). No
`EXECUTION_POLICY_SCHEMA_V1` bump (a bump would make `run_spec.rs` :433/:439 refuse every persisted run; the
additions decode on new binaries). Old binaries cannot decode new automatic rows: the accepted rollback limit
(§2 item 5).

**Work cutoff opt-in for watched workflows (I2b): PARKED.** Spec review rounds 1 and 2 each found new
consumers of the workflow work cutoff that misbehave when it is unarmed: the scheduler deadline (round 1),
the post-cancellation cleanup tail `min(cancel + 60 s, cutoff + 60 s)` (`execution_policy.rs` :1034,
`executor.rs` :5581; a watched run cancelled at 3 h would get zero cleanup time), and the critical-path /
retry-budget and fixed-grace admission validators that still use the nominal 2 h (`run_spec.rs` :445,
`execution_policy.rs` :1635). That is an open class. Before I2b gets another spec round, enumerate EVERY
consumer of `work_cutoff_ms`, `effective_work_cutoff_ms`, `terminal_bound_ms` and `max_qualification`
(scheduler, arbitration and tie facts, cleanup tail, fixed grace, retry budget, terminal bound, warnings,
history/calibration, persistence) and give each an explicit armed / unarmed / explicit-duration semantics with
a test. The local `run-workflow` `client_watching` wiring belongs to I2b. **Until I2b ships, every automatic
workflow keeps today's always-armed 2 h cutoff** (watched or not), and gains only the silence cutoff.

### 4.5 Heartbeat

A `TurnLivenessBoard` (`Arc`) is injected through a new `AcpConfig.liveness_board` (set at `main.rs` :820,
and through `ContainerRwConfig` for `container_rw` agents, §4.3). The backend registers its tracker under
`(observer_key, turn_id)` at turn start with an RAII registration that removes exactly that entry on every
exit, including cancel and drop; several turns (workflow fan-out) can share one observer key. `snapshot(key)` returns one entry per in-flight turn: agent, elapsed, last-activity
age, open tools (title, kind, age), permission pending, paused, silence cutoff, work cutoff.

- Workflows: add a non-persisted `observer_key: Option<String>` to `WorkflowRunContext` (`executor.rs` ~:88).
  Local `run-workflow` sets it to the execution id (distinct from the attempt id that `run_id`/`task_id`
  carry, `main.rs` :4334/:4465). Served workflows set it to the served task id where the context is built
  (`crates/bridge-coordinator/src/coordinator.rs` ~:1790) and keep it in step where the detached runner
  replaces the task id (`detached.rs` ~:4047). The executor copies it into all three workflow `TurnMeta`
  constructions. The local CLI reads the board in-process.
- Served unary: the server sets `observer_key = routed.task`. A new bridge-private read-only RPC `TaskLiveness {taskId}` beside `AttemptStatus`
  (`server.rs` :1607), behind the same `authorize_headers`, returns `{turns: [...]}` or `TaskNotFound`.
- `submit` keeps unary `SEND_MESSAGE` (no output-contract change) and races it against an interval task
  polling `TaskLiveness(execution_id)` (the task id is `execution_id`, `main.rs` :6194). "Method not found"
  or any error falls back to an elapsed-only line. **Every poll has its own deadline** of
  `min(10 s, interval / 2)`; a poll that times out prints the elapsed-only line and the next beat polls again,
  so a stalled server can never silence the heartbeat. `run-workflow --serve` polls the same RPC and adds the
  last SSE frame's age.
- Format: one stderr line per beat, first at 60 s then every 60 s; nothing on stdout:
  `[heartbeat] elapsed=14m00s turns=1 agent=codex last_activity=2m10s open_tool="cargo test" tool_kind=execute tool_age=9m02s silence=paused cutoff=30m`
  or `[heartbeat] elapsed=14m00s liveness=unavailable`. Agent-controlled strings (tool title, kind) are
  truncated UTF-8-safely and then escaped JSON-style (quotes, backslash, newline, tab, and every control
  character including ESC), so one beat is always exactly one line and cannot drive the terminal.
- Control: `--heartbeat-secs N` (0 = off), `--no-heartbeat`, env `A2A_BRIDGE_HEARTBEAT_SECS`; a flag beats
  the env. On by default.
- **Watched = heartbeat on, and only where the run dies with its client** (§2 item 3). (Local `run-workflow`
  watched wiring is part of the parked I2b.) `submit` with the heartbeat on sends
  `a2a-bridge.client_watching=true` for a unary `message/send`; the server honours it only on that unary path,
  and only because dropping the unary request future cancels the ACP prompt: I3 must prove that with a test
  (the stub agent observes `session/cancel` after the client disconnects), and if it does not hold today I3
  adds the cancellation; honouring the flag without it is not allowed. The served workflow and detached paths
  ignore the flag. `--no-heartbeat` or 0 makes the run unwatched, so a script keeps the 2 h backstop.

## 5. Increments

I1 first; I2a and I3 each depend only on I1 and not on each other (the workflow `client_watching` wiring that
made I2 and I3 interdependent moved to the parked I2b). Each is one `a2a-bridge implement`
dispatch (Opus 5.5) with a two-round review cap. Local verification is fmt, clippy and tests for the touched
crates; the full workspace suite is CI's job and the merge gate.

**I1 — turn liveness engine and agent config.** Files: new `crates/bridge-core/src/turn_liveness.rs`;
`permission.rs` (TurnMeta field); `domain.rs` (`AgentLivenessConfig`, `AgentEntry.liveness`);
`execution_policy.rs` (`freeze_effect` trailing encoding only); `acp_backend.rs`; `bin/a2a-bridge/src/config.rs`
(`[agents.liveness]`, the shared duration parser); `main.rs` (AcpConfig forwarding);
`crates/bridge-container/src/lib.rs` (ContainerRwConfig liveness/board forwarding); `executor.rs` (only the
interim `Off`/`Off` at the three workflow turn sites); mechanical literal updates. Tests (each fails on
pre-change code; compile failure counts as structural only, so also show behavioural RED):
1. default config, agent silent 31 min → `acp.prompt.silence_cutoff`;
2. tool call `in_progress`, 40 min silence, then `completed` and end → completes;
3. `completed` for an unknown id does not pause (31 min trips); `in_progress` for an unknown id pauses;
4. identical-totals usage every 5 min trips at ~30 min after the last real activity;
5. changed totals every 20 min do not trip; 6. empty-text thoughts every 20 min do not trip;
7. a permission pending 40 min (raised permission timeout) does not trip; 31 min after resolution trips;
8. 65 tools opened then all 65 closed: 31 min of silence after the last close trips; 65 open with one left
   open: no trip; 4097 simultaneously open (small-cap seam): sticky overflow, trace emitted, no trip;
9. `silence_cutoff_secs = 0` never trips;
10. request/agent `work_cutoff` with a tool open trips `acp.prompt.work_cutoff`; unwatched default trips at
    2 h; `client_watching` default runs past 2 h;
11. legacy watchdog tests still return `acp.prompt.watchdog_timeout`;
12. automatic workflow admission accepts `[agents.liveness]`;
13. the provider-effect digest is unchanged without liveness config and changes with it;
14. two raw tool ids whose capped forms collide are tracked separately (closing one keeps the other open);
15. two pending permissions: resolving one keeps the clock paused, resolving both resumes it;
16. a tool closes and the turn stays open: 31 min of silence after the close trips (the unpause notify works);
17. usage keys: repeated `None` cost is unchanged, `None`→`Some` and a currency-only change count;
18. a `container_rw` agent with silence off stays active past 31 min (its inner config received the value);
19. a manual workflow turn stays active past 31 min (I1's interim `Off`);
20. the shared parser: 30 days ok, 30 days + 1 s and `u64::MAX` refused;
21. two maximal-length raw ids sharing the same 64-byte preview are tracked separately;
22. equal-instant deadlines report the fixed-priority cause.

**I2a — workflow silence.** Files: `execution_policy.rs` (`SilenceCutoffV1` variants, silence override,
resolution under `AutomaticR2f1b` only); `executor.rs` (map frozen silence onto the three turn sites, replacing
I1's interim `Off`); `config.rs` (`[[workflows]] silence_cutoff`); `main.rs` (`run-workflow --silence-cutoff`).
Stage the data carriers first so RED is behavioural. Tests: default automatic resolve → `AgentDefault`; manual
→ `None`, and the manual LITERAL, `MANUAL_WORKLOAD_FINGERPRINT_GOLDEN` and the `graph.rs` :568 shape golden
pass unchanged; a node silent 31 min ends `TimedOut` with cause `acp.prompt.silence_cutoff` and no retry even
with `node.retry`; workflow `silence_cutoff = "off"` never trips; an invocation duration overrides a
workflow `off`; a legacy persisted automatic controls literal (silence `none`) decodes, keeps no silence
cutoff, and re-encodes byte-identically; the automatic 2 h work cutoff is unchanged.

**I2b — opt-in workflow work cutoff for watched runs: PARKED** (§4.4). Not dispatched until its consumer
enumeration is written and reviewed.

**I3 — client heartbeat and per-request knobs.** Files: `main.rs` (heartbeat task and formatter, `submit`
flags, the local `run_workflow_cmd` loop, `run_workflow_serve_client`); `server.rs` (`TaskLiveness` RPC,
board ownership, `observer_key` on TurnMeta at :767/:2680/:3865, the watched-unary cancellation proof);
`params.rs` (metadata parse); `executor.rs` (`WorkflowRunContext.observer_key` into the three `TurnMeta`
sites); `crates/bridge-container/src/lib.rs` if the board path needs it. Tests: the
pure formatter (tool open, idle, unavailable); flag/env precedence and `--no-heartbeat` silence; a paused-time
loop writes 2 lines for a 150 s wait into a captured writer and nothing to stdout; `TaskLiveness` returns an
open tool for an in-flight stub turn, `TaskNotFound` when idle, and requires auth; an older server ("method
not found") falls back to elapsed-only; metadata `"off"` → Off and `"abc"` → `InvalidRequest`;
heartbeat on sends `client_watching=true`, off does not; a `TaskLiveness` poll the stub server holds forever
still yields elapsed-only beats on the following intervals; titles with newline, quote, ESC and a multibyte
truncation boundary format as one escaped line; a served workflow's turns are found by `TaskLiveness` under
its task id; a local workflow with distinct execution and attempt
ids registers under the execution id (the attempt-id snapshot is empty) and the CLI prints detailed lines; the
board keeps two fan-out turns under one key, removes exactly the finished one, cleans up on cancel, and is
empty after both; a watched unary turn is cancelled (stub observes `session/cancel`) when the client
disconnects; `client_watching` on a served workflow request is ignored (cap armed). Files also include
`crates/bridge-coordinator/src/coordinator.rs` and `detached.rs` (served-workflow observer key).

Docs with the increments: `CHANGELOG.md`, `docs/onboarding.md` or AGENTS.md wording for the new knobs, and the
operator SERVICE.md rollback note (operator-side, by the controller).

## 6. Accepted risks

- A hung tool in a watched unary turn runs until the client cancels (owner decision). Workflows keep the 2 h
  cutoff until I2b ships.
- Detached/batch/served-workflow runs keep the 2 h backstop by default (they outlive their client), so a hung
  tool there still ends unless the caller explicitly opted out.
- New automatic run-policy rows are unreadable to older binaries (rollback limit, owner decision).
- Every new automatic run's controls fingerprint changes (re-partitions automatic history/calibration);
  manual fingerprints do not move.
