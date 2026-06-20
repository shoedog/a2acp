# Slice 7 — Rich ACP observability + E9 watchdog: ARCHITECTURE ANALYSIS (controller pass)

> Elaborates the CONVERGED design-of-record (do NOT re-litigate the decomposition): architecture spec
> `2026-06-17-orchestration-architecture.md` (P-3 rich `session/update`, P-4 stop-reasons, the WATCHDOG / UPDATE-
> MINIMAL invariants) + slicing spec `2026-06-17-orchestration-slicing.md` row 7. Controller's analysis pass; a
> parallel codex-xhigh pass runs for dual-lens convergence (per the Slice-5/6 pattern). Builds on
> `slice-6-journal-shipped`; resume via the orchestration HANDOFF.

## What Slice 7 is (slicing row 7)
**Rich ACP observability + E9 watchdog.** Two coupled parts (J→I,E dep — the watchdog is meaningful only once the
journal carries liveness):
- **Rich mapping:** capture the `session/update` notifications the bridge DROPS today — `plan` (complete-replace),
  `tool_call`/`tool_call_update` (patch by `tool_call_id`), `current_mode_update`, `config_option_update`,
  `available_commands_update` — map to **bridge-owned `OrchEventKind` variants** (NOT raw SDK enums), JOURNAL them
  (the Slice-6 substrate), and surface them in the `task watch` transcript.
- **E9 watchdog:** fire on **"no JOURNAL event for N s"** per active turn (idle timeout vs a hard wall-clock),
  pending permission counts as activity, a long `in_progress` tool_call (the FN-1 `_dyld_start` shape) must NOT
  false-trip.
- **DoD (roadmap):** tool/plan/config events visible in the transcript; a deliberately-hung turn is caught; a long
  `in_progress` tool_call does NOT false-trip. **OUT:** permission DECISIONS (S9); stop-reason→OrchResult (P-4, no
  S7 consumer).

## Design-of-record (binding rulings — do NOT reopen)
- **P-3:** add first-class `OrchEventKind` variants `Plan`/`ToolCall`/`ToolCallUpdate` + config/mode/commands.
  **`Plan` = COMPLETE-REPLACEMENT** (SDK: "the client replaces the entire plan with each update" — schema 0.13.2
  `Plan{entries:Vec<PlanEntry{content,priority,status}>}`); **`ToolCallUpdate` = PATCH** over a prior `tool_call`
  keyed by **`tool_call_id`**. **Bridge-owned DTOs** (SDK shapes shift under `unstable_*` feature flags →
  persisting raw SDK enums couples replay to SDK drift).
- **UPDATE-MINIMAL (invariant):** `Update` grows ONLY to `Usage`; `Plan`/`ToolCall`/config/mode/commands stay
  **journal-level**, adapted INSIDE the ACP turn/event adapter (the `map_session_update` site,
  `acp_backend.rs:1631`), **NEVER pushed into the backend `Update` port.** So the rich events bypass `Update` and
  reach the journal via a SIDE-CHANNEL.
- **WATCHDOG:** fire on no-JOURNAL-event-for-N-s (any `ToolCall`/`ToolCallUpdate`/`Plan`/`Usage`/text = liveness);
  pending permission = activity; **separate idle timeout from hard wall-clock** (the no-chunk long model turn =
  residual → the `turn_kill` backstop `acp_backend.rs:297/1606`).
- **Dual store (S6, holds):** typed columns = W3b resume (node-lifecycle ONLY, no serde); serialized `OrchEvent`
  journal rows = the rich journal (Ser+De). **One shared `next_seq` (`last_event_seq`), never a parallel cursor.**

## Current state (ground-truth map — controller-verified)
- **The drop site:** `map_session_update(notif) -> Option<Update>` (`acp_backend.rs:1631-1654`) handles ONLY
  `AgentMessageChunk→Text` + `UsageUpdate→Usage`; everything else `_ => None` (dropped). The SDK
  (`agent-client-protocol =0.12.1`, schema `0.13.2`) DOES expose `SessionUpdate::{ToolCall, ToolCallUpdate, Plan,
  AvailableCommandsUpdate, CurrentModeUpdate, ConfigOptionUpdate}` (`schema/v1/client.rs:91-113`).
- **Turn flow + the side-channel gap:** the SDK `on_receive_notification` handler (`acp_backend.rs:963`) runs on the
  SDK event loop, calls `map_session_update`, pushes `Some(Update)` to a per-turn mpsc that the driver task
  (`:1775-1855`) drains into the returned `BackendStream`. The handler has the `session`, NOT `task_id`/`op`/
  `TaskStore`. **There is NO existing channel for journal-level rich events** — they're swallowed at `_ => None`.
- **The S6 journal write (just merged):** `insert_journal_event` (`sqlite.rs:185`) inside the sequenced
  `unchecked_transaction`; seq via `UPDATE tasks SET last_event_seq=last_event_seq+1` → SELECT. The 3 writers
  (record_node_started / put_node_checkpoint_sequenced / set_terminal_sequenced) are the ONLY journal writers, all
  driven by `DetachedProgressSink` (`workflow_sink.rs:101/119/151`) at NODE boundaries + `finalize_detached`. **NO
  intra-turn journal path exists.** The sink holds `{store, task, op=op-<task>}`.
- **The transcript wire:** `FrameKind ∈ {NodeStarted, NodeFinished, SnapshotComplete, Terminal}` (`reattach.rs:39`),
  serialize-only; `snapshot_frames` (`server.rs:1257`) builds from the folded `TaskProgressSnapshot`;
  `fold_journal_to_snapshot` (S6) projects ONLY node kinds (ignores Progress/Usage). `task_watch_cmd`
  (`main.rs:3022`) is a dumb pass-through (ignores unknown `kind`).
- **Watchdog machinery (partial):** per-session `turn_lock` (`acp_backend.rs:288`, held for the turn) + `turn_kill`
  notify slot (`:306`, fresh per turn `:1763`, fired by `cancel()` after `cancel_grace` `:1574`). NO per-turn
  `last_activity_ms`; NO session↔task↔op correlation in the backend; NO `awaiting_permission` flag (auto-approve
  replies in-turn, never produces `Update::Permission` — confirmed `acp_backend.rs:155`).

## The central architectural challenge — the rich-event journal path
Rich events are produced at the backend (`map_session_update`, SDK event loop) but must be journaled with a `seq`
from the **TaskStore** (inbound/store layer, ABOVE the backend) under the running node's **`op-<task>`**. Three plumbing options (the pivotal D-A):
- **(A) Dependency-inverted rich sink** — define an `Arc<dyn RichEventSink { record(OrchEventKind) }>` (in
  bridge-acp/bridge-core, bridge-owned-DTO types), plumb a per-turn instance into `backend.prompt`/the turn; the
  handler adapts SDK→`OrchEventKind` and calls it; the inbound impl journals (it has `{store, task, op}`). Mirrors
  the S5 `WorkflowNodeDispatcher` seam (executor defines the trait, inbound implements). The backend stays journal-
  ignorant (just an adapter + a sink call).
- **(B) Side-stream on the prompt return** — `prompt` returns the `Update` stream PLUS a rich-event receiver; the
  caller (executor turn-drain / `DetachedProgressSink`, which has `{store, task, op}`) drains both and journals.
  Backend stays journal-ignorant; but the `prompt`/`AgentBackend` API surface changes for EVERY backend impl
  (Acp/ContainerRw/Api) + the executor drain.
- **(C) Per-turn broadcast** — a broadcast the caller subscribes to (a variant of B).
**Controller lean = (A)** (matches the proven S5 dependency-inversion seam; localizes the change to the ACP adapter
+ one inbound impl; the other backends simply don't emit rich events). The sink is a NO-OP for the streaming/warm
path (no seq substrate); LIVE only for detached workflow nodes.

**The seq + the new writer (D-B):** rich events are **JOURNAL-ONLY** (no typed-column row — typed stays node-
lifecycle/W3b-resume). Add `record_event_sequenced(task, op, ts, kind) -> seq` (allocates a `last_event_seq`, writes
ONE `task_journal` row, NO typed write). So a node's turn journals `NodeStarted(k) · Plan(k+1) · ToolCall(k+2) ·
ToolCallUpdate(k+3) … · NodeFinished(k+n)`, all under `op-<task>`, one cursor. Single-writer-per-task holds (one
detached runner). **Confirm:** does the W3b fold/resume IGNORE rich rows? (`fold_journal_to_snapshot` already
ignores non-node kinds; resume reads typed checkpoints — so yes, rich rows are inert to resume.)

**The transcript projection (D-C):** the snapshot fold must now ALSO project rich events: **Plan = latest-wins**
(keep only the last `Plan` row in the snapshot); **ToolCall+ToolCallUpdate = patch** (fold updates into each
`tool_call`'s current state keyed by `tool_call_id`); node lifecycle still collapses (S6). New
`FrameKind::{Plan, ToolCall, ToolCallUpdate, …}` (additive — `task_watch` ignores unknown kinds, BUT the byte
stream is now richer → **this IS the "deferred wire change" S6 punted to S7**; S6's golden EXTENDS, not breaks).
Live tail = each rich event = one frame as it happens (no folding); snapshot = the folded current state.

## Controller slicing strategy (for dual-lens convergence)
**Split into two shippable slices (J→I,E):**
- **S7a — Rich ACP event journaling + transcript.** The bridge-owned rich `OrchEventKind` DTOs + the
  `map_session_update` adapter for the 6 dropped variants + the `RichEventSink` seam (D-A) + `record_event_sequenced`
  (D-B) + the snapshot/live projection with Plan-replace/ToolCall-patch + new `FrameKind`s (D-C). DoD: a detached
  workflow run's tool/plan/config events are journaled + visible in `task watch`; W3b resume + the S6 byte-frozen
  NODE frames intact (rich frames are additive).
- **S7b — E9 watchdog.** A per-active-detached-turn liveness monitor reading the journal's last-event ts (or a
  per-op `last_activity_ms` bumped on each journal write); idle-timeout vs hard-wall-clock; pending-permission =
  activity; cancel via the existing `request_cancel`+grace+`turn_kill`. DoD: a deliberately-hung turn is caught; a
  long `in_progress` tool_call does NOT false-trip; the no-event long model turn hits the wall-clock backstop.

**Open decisions for the dual-lens pass (where the controller wants a second opinion):**
- **D-A (the side-channel — THE pivotal call):** dependency-inverted `RichEventSink` (A) vs prompt-return side-stream
  (B) vs broadcast (C)? Where does the sink trait live (bridge-acp defines it over bridge-core DTOs?), and how does
  `{task, op}` reach the per-turn sink (via `NodeTurn`/the `WorkflowNodeDispatcher` checkout, which already plumbs
  per-node context)? Does the backend stay 100% journal-ignorant?
- **D-B (rich = journal-only + the new writer):** is `record_event_sequenced` (journal-only, no typed write) the
  right shape? Confirm it shares `last_event_seq` atomically and that W3b resume/fold is inert to rich rows. Any
  ordering hazard interleaving rich writes with node-lifecycle writes (same single-writer)?
- **D-C (the transcript projection + the wire change):** new `FrameKind`s vs a richer projection; the Plan-replace /
  ToolCall-patch FOLD in the snapshot (keyed by `tool_call_id`); does S6's golden extend cleanly (node frames
  unchanged, rich frames additive)? Is the live tail raw pass-through while the snapshot folds? Does `journal_fold
  _inputs`/`fold_journal_to_snapshot` extend, or a new richer projection fn?
- **D-D (scope of rich journaling):** detached workflow nodes ONLY (same seq-substrate constraint as S6)? The
  warm/streaming/unary path has no TaskStore seq → rich events there are dropped (or a later slice). The `{task,op}`
  plumbing to the backend turn.
- **D-E (watchdog design, S7b):** where does it live (a server-side per-op sweeper reading the journal ts, vs a
  per-turn deadline bumped on each journal write)? idle vs wall-clock knobs; how "pending permission = activity" is
  detected for a FUTURE non-auto policy (an `awaiting_permission` flag) given today's auto-approve is instant; the
  cancel path (reuse `turn_kill`?). The session↔task↔op correlation it needs.
- **D-F (slicing granularity):** is S7a + S7b right, or fold into one? Is S7a itself too big (rich-DTO+adapter vs
  the projection/wire change as separate units)?

## DUAL-LENS CONVERGENCE (controller + codex-xhigh, both `sound-with-changes`)
codex corroborated the architecture (S7a→S7b split, `RichEventSink` dependency inversion, journal-only rich rows,
new FrameKinds, per-turn watchdog) and added CRITICAL corrections (controller-verified). Binding outcome:

**Slicing (CONFIRMED + refined):** S7a (rich journaling + transcript) → S7b (watchdog); do NOT fold. **S7a sequences
INTERNALLY: (1) side-channel first** (rich DTOs + ACP adapter + nonblocking sink + journal-only writer + the FLUSH
barrier), **(2) transcript projection/wire second.** Highest rework-risk unit = the side-channel plumbing → pin it
first.

**CORRECTION-1 (material — the controller's plumbing assumption was WRONG):** the DETACHED runner does NOT use the
`WorkflowNodeDispatcher` seam — `spawn_detached_workflow` calls `executor.run_from_with_context` directly
(`server.rs:2096`); only the WARM SSE path uses `run_with_context_and_dispatcher` + `WarmWorkflowNodeDispatcher`
(`server.rs:1935`). So the rich sink CANNOT ride the dispatcher checkout for detached. → **D-A = A-prime:** a
core-owned `RichEventSink` trait in `bridge-core::ports` (over `bridge-core::orch` DTOs) + a **DEFAULTED
`AgentBackend::prompt_observed(session, parts, sink)`** method (default = call `prompt`, ignore sink — so API/
ContainerRw/replay/tests are untouched); ACP overrides it. The detached runner builds a per-node-turn sink (it has
`{store, task, op-<task>}`) and threads it through `run_from_with_context` → the node turn → `prompt_observed`.
NOT a mandatory `prompt` signature rewrite (B would touch every backend + many tests).

**CORRECTION-2 (the FLUSH BARRIER — load-bearing for ordering):** the handler is non-blocking (`acp_backend.rs:963`
"runs ON the event loop, must NEVER call `cx`/block" — it `try_record`s into a per-turn NONBLOCKING queue, NEVER
writes the store); the OFF-LOOP driver/sink does the async journal write. Without a barrier, queued rich rows can
commit AFTER `NodeFinished`/Terminal (writer lag). → the executor/sink MUST **`flush().await` the node's rich queue
BEFORE emitting `NodeFinished`/terminal**, so a node's rich rows always precede its `NodeFinished` in seq order.

**D-B (CONFIRMED):** add `record_event_sequenced(task, op, ts, source?, kind) -> seq` — JOURNAL-ONLY (allocates
`last_event_seq`, writes ONE `task_journal` row, NO typed column). SQLite already does the one-tx seq+journal at
`sqlite.rs:593/650/711`; Memory at `task_store.rs:338`. W3b inert (resume seeds typed checkpoints `server.rs:2329`;
the fold ignores non-node kinds `task_store.rs:263`). Caveat: per-turn queues make the writer multi-producer — seq
stays atomic, but ordering correctness RELIES on the per-node flush-before-NodeFinish (CORRECTION-2).

**D-C (refined — do NOT overload `TaskProgressSnapshot`):** `TaskProgressSnapshot` is node-resume state; keep it
node-only (don't make resume carry observability). Add a **SEPARATE rich transcript projection** over
`journal_fold_inputs`: **latest Plan** (complete-replace), **folded ToolCall current state keyed by `tool_call_id`**
(ToolCallUpdate is a SPARSE PATCH — only `tool_call_id` required, the rest Optional; content/locations REPLACE whole
collections — `v1/tool_call.rs:169/217`), latest config/mode/commands. New `FrameKind::{Plan,ToolCall,…}` (additive;
`task_watch` is a dumb pass-through). **Live tail = each committed rich row → one raw frame; snapshot = the fold.**
The rich snapshot projection is REQUIRED (not optional) for lossless reconnect after broadcast lag
(`reattach.rs:79`, `server.rs:1243`). **NODE frame shapes UNCHANGED, but node `seq`s SHIFT when rich rows interleave
→ the S6 byte-goldens stay valid ONLY for NO-RICH fixtures** (the existing `golden_two_node_run_wire_tuples` uses a
no-rich fixture → still valid; rich runs get NEW goldens).

**D-D (CONFIRMED):** detached workflow tasks ONLY in S7 (the seq substrate is detached-only — `workflow_sink.rs:75`;
warm/unary records usage in `SessionManager` with no TaskStore cursor — `server.rs:1395`). Rich events on warm/unary
paths = no-op until a later session-stream substrate.

**D-E (refined — PER-ACTIVE-NODE-TURN, not a DB sweeper):** workflows run sibling nodes CONCURRENTLY (`FuturesUnordered`
`executor.rs:412/455`) → a task-level "last journal event" lets one chatty node MASK a hung sibling. The watchdog is
per-active-node-turn; it needs the active backend/session to cancel (the journal doesn't carry that) → reuse
`backend.cancel(session)` (ACP sends `session/cancel` + arms `turn_kill` after grace, `acp_backend.rs:1906`). TWO
knobs in a NEW watchdog config namespace: **idle-timeout** (reset on each committed liveness event) + **hard
wall-clock** (from turn start) — NOT the warm idle-TTL or ACP cancel-grace (different lifetimes). NO
`awaiting_permission` state in S7b (auto-approve is instant + off-loop `acp_backend.rs:1007`; non-auto pending = S9 —
the handler MAY count a permission request as activity). **Watchdog-liveness gap (codex):** text/usage are NOT
journaled in detached nodes today (executor concatenates text + ignores usage `executor.rs:164/166`) → rich events
(Plan/ToolCall) provide liveness; a text-only / no-chunk turn → the hard wall-clock backstop (don't journal every
text chunk).

**D-F:** S7a + S7b confirmed; S7a = side-channel first, projection second.

**Adopted hazards (codex):** (1) the flush barrier (CORRECTION-2). (2) detached bypasses the dispatcher
(CORRECTION-1). (3) no store writes from ACP handlers (block the SDK loop). (4) text/usage liveness gap. (5)
broadcast-lag lossless-reconnect needs the rich snapshot fold. (6) ToolCallUpdate sparse-patch / orphan-update
handling. (7) **`event_json` BALLOON** — diffs carry `old_text`/`new_text`, `raw_input`/`raw_output` are arbitrary
JSON (`v1/tool_call.rs:576/49`) → CAP/TRUNCATE before journaling (the bridge DTO records a SUMMARY, not raw payloads).

**Cut/defer:** S7a core = **Plan + ToolCall + ToolCallUpdate** (+ whatever activity the watchdog needs); config/
mode/commands sequenced AFTER the core path but kept in S7. DEFER: warm/unary rich journaling, permission-awaiting
state (S9), command-invocation semantics, `session_info_update`, full UI rendering beyond JSON frames, raw unbounded
tool-payload persistence.

## DoD / gate (roadmap)
S7a: a detached run emits `tool_call`/`plan`/`config` journal rows → `task watch` shows them (new frame kinds) →
W3b resume + S6 node-frame byte-identity intact. S7b: a deliberately-hung turn (no journal event) is cancelled by
the idle/wall-clock watchdog; a long `in_progress` tool_call (steady `ToolCallUpdate`s) does NOT false-trip.
Live-gate vs real codex (codex emits plan + tool_call events natively).
