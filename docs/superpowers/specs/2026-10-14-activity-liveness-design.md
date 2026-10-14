# Activity liveness: silence cutoff, opt-in hard cap, client heartbeat (design)

**Status:** draft for Sol/xhigh spec review (cap two rounds). Issues #47 and #24.
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
   heartbeat of item 4. Any run can explicitly turn the cap on or off.
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
- **except** a `UsageUpdate` whose totals tuple `(used, size, cost.amount.to_bits(), cost.currency)` equals
  the previous usage update's tuple in the same turn. The first usage update counts. Compare with the previous
  update, not a high-water mark, so a drop after compaction counts. `TurnProgress.usage` is unchanged. Final
  usage arrives with the prompt response, which ends the turn.
- a permission request's arrival and its resolution both count.

**Open tool calls**, kept in the tracker as a map keyed by the **raw** `toolCallId` (not the capped id from
`map_session_update_rich`, which can collide), value `{title ≤ 128 bytes, kind, opened_ms}`:
- `ToolCall` or `ToolCallUpdate` with status `pending` or `in_progress` opens the id (or refreshes it);
  `completed` or `failed` closes it; any other status, or no status, changes nothing. An update for an
  unknown id with `pending`/`in_progress` opens it (title/kind from the update, else `"?"`); an unknown id
  with `completed`/`failed` is a no-op. All of these are activity.
- At most 64 open entries. A 65th open sets a sticky `open_overflowed` for the rest of the turn and emits
  a trace event `acp.liveness_tool_set_overflow`. Overflow counts as paused (fails open: never a false kill).
- `TurnProgress.tool_states` stays as it is.

**Pending permissions:** a counter incremented on arrival (`acp_backend.rs` :4499) and decremented by an RAII
guard moved into the permission task around `resolve_permission_outcome`, so it runs on every path
including drop.

**Clock:** `paused = open non-empty || open_overflowed || pending_permissions > 0`.
`anchor = max(last_activity_ms, last_unpause_ms)`; silence is due at `anchor + silence_ms` while not
paused. The timer sleeps until the earliest of silence-due, turn-work-due and the legacy deadlines; it is
woken on turn end or by a `Notify` raised on paused→unpaused transitions; it re-checks under the tracker
lock before firing.

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
  Their silence value comes from the frozen workflow controls (§4.4).
- Both config values are validated ≤ 30 days (reuse the watchdog cap at `config.rs` :2196).
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

**Work cutoff arming:** add to `FrozenWorkflowControlsV1` (:476)
`#[serde(default, skip_serializing_if = "Option::is_none")] work_cutoff_arming: Option<WorkCutoffArmingV1>`,
`Unarmed | Armed { work_cutoff_ms, source }`.
- `None` (absent) = legacy: armed at the frozen value when automatic. Persisted rows keep their behaviour and
  re-encode byte-identically.
- Manual activation leaves it `None` (bytes unchanged).
- Automatic: `Armed` at 2 h unless the invocation is watched; watched → `Unarmed`, unless the run opted in.
- Knobs: `[[workflows]] work_cutoff_ms` (add explicitly: `WorkflowToml` is `deny_unknown_fields`),
  `ExecutionPolicyInvocationV1.work_cutoff_ms` and `.client_watching`, `run-workflow --work-cutoff <secs|off>`
  (replacing `::default()` at `main.rs` :4624). Precedence: invocation > workflow > watched default.
- A present `max_qualification` keeps its meaning and counts as opt-in at its value; setting both it and
  `work_cutoff_ms` is a new `ExecutionPolicyError::ConflictingWorkCutoff`.
- Accessors `armed_work_cutoff_ms() -> Option<u64>` and `armed_terminal_bound_ms()`; keep
  `effective_work_cutoff_ms()` nominal, because the retry budget (`run_spec.rs` :445), fixed grace ≤ cutoff
  (:1635) and `effective_terminal_bound_ms` depend on it.
- Executor: the one behaviour change is `scheduler_work_cutoff_ms = armed_work_cutoff_ms().unwrap_or(u64::MAX)`
  (`executor.rs` :5560). Confirm in tests that the cleanup deadline after cancellation, fixed grace and tie
  facts saturate safely with `u64::MAX`.
- No `EXECUTION_POLICY_SCHEMA_V1` bump (a bump would make `run_spec.rs` :433/:439 refuse every persisted run;
  the additions are decode-compatible on new binaries). Old binaries cannot decode new automatic rows: the
  accepted rollback limit (§2 item 5).

### 4.5 Heartbeat

A `TurnLivenessBoard` (`Arc`) is injected through a new `AcpConfig.liveness_board` (set at `main.rs`
:820/:1163). The backend registers its tracker under `TurnMeta.liveness.observer_key` at turn start and
removes it at turn end. `snapshot(key)` returns one entry per in-flight turn: agent, elapsed, last-activity
age, open tools (title, kind, age), permission pending, paused, silence cutoff, work cutoff.

- Local `run-workflow`: `observer_key = execution_id`; the CLI reads the board in-process.
- Served: the server sets `observer_key = routed.task` for unary turns; the executor uses the execution id
  for served workflows. A new bridge-private read-only RPC `TaskLiveness {taskId}` beside `AttemptStatus`
  (`server.rs` :1607), behind the same `authorize_headers`, returns `{turns: [...]}` or `TaskNotFound`.
- `submit` keeps unary `SEND_MESSAGE` (no output-contract change) and races it against an interval task
  polling `TaskLiveness(execution_id)` (the task id is `execution_id`, `main.rs` :6194). "Method not found"
  or any error falls back to an elapsed-only line. `run-workflow --serve` polls the same RPC and adds the last
  SSE frame's age.
- Format: one stderr line per beat, first at 60 s then every 60 s; nothing on stdout:
  `[heartbeat] elapsed=14m00s turns=1 agent=codex last_activity=2m10s open_tool="cargo test" tool_kind=execute tool_age=9m02s silence=paused cutoff=30m`
  or `[heartbeat] elapsed=14m00s liveness=unavailable`.
- Control: `--heartbeat-secs N` (0 = off), `--no-heartbeat`, env `A2A_BRIDGE_HEARTBEAT_SECS`; a flag beats
  the env. On by default.
- **Watched = heartbeat on.** When the CLI's heartbeat is on, it sends `client_watching=true` (metadata for
  served, invocation field for local). `--no-heartbeat` or 0 makes the run unwatched, so a script that turns
  the heartbeat off keeps the 2 h backstop.

## 5. Increments

Increment 1 first; 2 and 3 each depend on 1 and not on each other. Each is one `a2a-bridge implement`
dispatch (Opus 5.5) with a two-round review cap. Local verification is fmt, clippy and tests for the touched
crates; the full workspace suite is CI's job and the merge gate.

**I1 — turn liveness engine and agent config.** Files: new `crates/bridge-core/src/turn_liveness.rs`;
`permission.rs` (TurnMeta field); `domain.rs` (`AgentLivenessConfig`, `AgentEntry.liveness`);
`execution_policy.rs` (`freeze_effect` trailing encoding only); `acp_backend.rs`; `bin/a2a-bridge/src/config.rs`
(`[agents.liveness]`); `main.rs` (AcpConfig forwarding); mechanical literal updates. Tests (each fails on
pre-change code; compile failure counts as structural only, so also show behavioural RED):
1. default config, agent silent 31 min → `acp.prompt.silence_cutoff`;
2. tool call `in_progress`, 40 min silence, then `completed` and end → completes;
3. `completed` for an unknown id does not pause (31 min trips); `in_progress` for an unknown id pauses;
4. identical-totals usage every 5 min trips at ~30 min after the last real activity;
5. changed totals every 20 min do not trip; 6. empty-text thoughts every 20 min do not trip;
7. a permission pending 40 min (raised permission timeout) does not trip; 31 min after resolution trips;
8. 65 open tools: no trip, overflow trace emitted; 9. `silence_cutoff_secs = 0` never trips;
10. request/agent `work_cutoff` with a tool open trips `acp.prompt.work_cutoff`; unwatched default trips at
    2 h; `client_watching` default runs past 2 h;
11. legacy watchdog tests still return `acp.prompt.watchdog_timeout`;
12. automatic workflow admission accepts `[agents.liveness]`;
13. the provider-effect digest is unchanged without liveness config and changes with it.

**I2 — workflow policy.** Files: `execution_policy.rs`; `executor.rs`; `config.rs` (WorkflowToml knobs);
`main.rs` (`run-workflow --work-cutoff/--silence-cutoff`, `client_watching` into the invocation); affected
tests and fixtures (`executor.rs` :15362 must opt in). Tests: default automatic + watched →
`armed_work_cutoff_ms() == None` and `AgentDefault`; unwatched → armed at 2 h; an executor run that is watched
passes 2 h + 1 ms with no cancel; an opt-in cutoff fires at its value; a legacy persisted automatic controls
literal (no arming field) decodes, behaves armed at 2 h, and re-encodes byte-identically; a node silent
31 min ends `TimedOut` with cause `acp.prompt.silence_cutoff` and no retry even with `node.retry`; workflow
silence off never trips; `work_cutoff_ms` + `max_qualification` → `ConflictingWorkCutoff`; the three manual
goldens pass unchanged.

**I3 — client heartbeat and per-request knobs.** Files: `main.rs` (heartbeat task and formatter, `submit`
flags, the local `run_workflow_cmd` loop, `run_workflow_serve_client`); `server.rs` (`TaskLiveness` RPC,
board ownership, `observer_key` on TurnMeta at :767/:2680/:3865); `params.rs` (metadata parse). Tests: the
pure formatter (tool open, idle, unavailable); flag/env precedence and `--no-heartbeat` silence; a paused-time
loop writes 2 lines for a 150 s wait into a captured writer and nothing to stdout; `TaskLiveness` returns an
open tool for an in-flight stub turn, `TaskNotFound` when idle, and requires auth; an older server ("method
not found") falls back to elapsed-only; metadata `"off"` → Off and `"abc"` → `InvalidRequest`;
heartbeat on sends `client_watching=true`, off does not.

Docs with the increments: `CHANGELOG.md`, `docs/onboarding.md` or AGENTS.md wording for the new knobs, and the
operator SERVICE.md rollback note (operator-side, by the controller).

## 6. Accepted risks

- A hung tool in a watched run runs until the client cancels (owner decision).
- Detached/batch/served runs with no watching client keep the 2 h backstop, so a hung tool there still ends.
- New automatic run-policy rows are unreadable to older binaries (rollback limit, owner decision).
- Every new automatic run's controls fingerprint changes (re-partitions automatic history/calibration);
  manual fingerprints do not move.
