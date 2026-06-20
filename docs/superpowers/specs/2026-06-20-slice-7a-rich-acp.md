# Slice 7a — Rich ACP event journaling + transcript — Spec

> **Status:** v2 (dual spec-review folded — codex-xhigh + Opus, both `fix-then-plan`; FIX-1..13 below are BINDING
> and SUPERSEDE contradicting body text). Architecture SOUND; the executable seams were corrected. Next: plan →
> dual plan-review.
> **Companion analysis (binding rulings):** `docs/superpowers/specs/2026-06-20-slice-7-rich-acp-ANALYSIS.md`
> (DUAL-LENS CONVERGENCE). **Design-of-record:** `2026-06-17-orchestration-architecture.md` P-3 + the UPDATE-MINIMAL
> / WATCHDOG invariants. **Roadmap:** slicing row 7 (split S7a rich-journaling → S7b watchdog). Builds on
> `slice-6-journal` (the journal substrate, merged `9df6be4`).

## Goal
Capture the rich ACP `session/update` notifications the bridge DROPS today — **`plan`** (complete-replace),
**`tool_call`** / **`tool_call_update`** (patch by `tool_call_id`) — for a DETACHED workflow run, map them to
**bridge-owned `OrchEventKind` DTOs**, JOURNAL them on the Slice-6 substrate (one shared `last_event_seq`), and
surface them in the `task watch` transcript (new additive `FrameKind`s). This makes the journal carry intra-turn
liveness — the substrate S7b's watchdog needs. (config/mode/commands events are sequenced AFTER the core path but
still land in S7a; see §Scope.)

## v2 — dual spec-review fixes folded (BINDING; SUPERSEDES contradicting body text)
Dual spec-reviewed (codex-xhigh + Opus, BOTH `fix-then-plan`; architecture SOUND, executable seams need
correction). FIX-1..13 are binding; read them FIRST.
- **FIX-1 (BLOCKER) — a SIBLING rich mapper, not an extension of `map_session_update`.** `map_session_update`
  returns `Option<Update>` (`acp_backend.rs:1631`) and CANNOT carry a rich `OrchEventKind` (UPDATE-MINIMAL forbids
  an `Update::Rich`). Add a separate pure `map_session_update_rich(&SessionNotification) -> Option<OrchEventKind>`
  (borrow, or restructure — `map_session_update` consumes `notif` by value). The handler (`acp_backend.rs:976`)
  tries the existing mapper → `TurnEvent::Text/Usage`, ELSE the rich mapper → `TurnEvent::Rich(kind)`.
- **FIX-2 (BLOCKER) — `WorkflowRunContext` carries a per-node SINK FACTORY, not a shared sink.** `WorkflowRunContext`
  is `#[derive(Clone)]` and CLONED into every concurrent sibling node future (`executor.rs:452`) — a shared
  `Option<Arc<dyn RichEventSink>>` would be one sink across siblings (cross-node `flush`, wrong). → carry
  `make_rich_sink: Option<Arc<dyn RichEventSinkFactory>>` where `RichEventSinkFactory::make(&NodeId, &OperationId)
  -> Arc<dyn RichEventSink>` (Send+Sync); `run_node` instantiates a FRESH per-node sink from it before
  `prompt_observed`. The factory holds `{store, task, op, hub}`. (Scheduler/topo MUST NOT read it — only `run_node`,
  per the `WorkflowRunContext` doc invariant.)
- **FIX-3 (BLOCKER) — swap at the NON-DISPATCHER prompt site (`executor.rs:229`).** `run_node` has TWO `prompt`
  sites: the dispatcher branch (`:144`, WARM — out of scope) and the cold/non-dispatcher branch (`:229`, which the
  DETACHED runner uses — `run_from_with_context` passes `dispatcher=None`). Instrument ONLY `:229`:
  `resolved.backend.prompt_observed(&session, vec![Part{text:rendered}], sink)` when the factory is present (else
  `prompt`).
- **FIX-4 (BLOCKER) — `DetachedRichSink { store, task, op, hub, queue }` (needs the HUB).** Live frames publish to
  the `TaskProgressHub` (owned by the detached path, in scope at `server.rs:2063`); `{store,task,op}` alone can
  write durable rows but NOT publish. `flush()` writes each queued row via `record_event_sequenced` → gets `seq` →
  publishes the rich `WorkflowProgressFrame{seq,phase:Live,…}` to the hub (durable-THEN-publish, like S6 node frames).
- **FIX-5 (MAJOR) — the `unfold` gets a SKIP-RICH LOOP + the captured sink.** The driver unfold (`acp_backend.rs:1859`)
  maps one `TurnEvent`→one `Update`. `TurnEvent::Rich` must NOT yield an `Update`. `prompt_observed` builds a
  sink-aware stream (`prompt_inner(..., rich_sink)`); the unfold body LOOPS: `Some(Rich(kind)) => { sink.record(kind);
  continue }`, `Text/Usage/Done/Failed =>` yield/finish as today. The sink is captured into the unfold closure (the
  unfold currently owns only `(rx, done)`).
- **FIX-6 (MAJOR) — the flush barrier is INSIDE `run_node`, not in `DetachedProgressSink`.** `NodeFinished` is
  written by the inbound `DetachedProgressSink` AFTER `run_node` returns (the rich sink/stream are out of scope
  there). Put `sink.flush().await` in `run_node`'s non-dispatcher branch AFTER the drain loop, BEFORE
  `forget_session`+`return (text, ok)` (`executor.rs:~257`). `run_node` completing precedes the `yield
  WorkflowEvent::NodeFinished`, so the node's rich rows commit first.
- **FIX-7 (MAJOR) — the merged snapshot projection is a NEW fn, at BOTH reattach sites.** `snapshot_frames`
  (`server.rs:1277`) consumes node-only `TaskProgressSnapshot`; `fold_or_typed_snapshot` (`:1102`) returns node-only
  state. Add a NEW projection reading `journal_fold_inputs(task).events` → a seq-ordered `Vec<(seq, FrameKind)>`
  merging node frames AND folded rich frames; REPLACE `snapshot_frames` at BOTH call sites (`server.rs:1054` reattach
  + `:1194` working_sse), preserving the `SnapshotComplete` sentinel seq + the `dedup_floor`/`cut_seq` (`:1205`).
  Per `tool_call_id`: emit ONE `ToolCall` (merged state) at the LAST-applied seq when a base `ToolCall` exists; emit
  `ToolCallUpdate` at the update seq for an ORPHAN update (no prior `ToolCall`). Keep only the LATEST `Plan`.
- **FIX-8 (MAJOR) — DEFER config/mode/commands; S7a CORE = Plan + ToolCall + ToolCallUpdate.** Their DTOs were
  factually wrong (`ConfigOptionUpdate.config_options: Vec<SessionConfigOption>`, `AvailableCommandsUpdate.available
  _commands: Vec<AvailableCommand{name,description,input}>` — NO `id`, `CurrentModeUpdate.current_mode_id`), they add
  real mapper/cap surface, and they're NOT gate-critical (DoD = plan+tool_call). → CUT from S7a → a fast-follow
  (S7a-config). Removes the §Data-model `ModeUpdate`/`ConfigUpdate`/`CommandsUpdate` variants.
- **FIX-9 (MAJOR) — precise DTOs for the 3 kept variants.** SDK: `ToolCallContent` is a 3-variant enum
  (`Content(ContentBlock)`/`Diff{path,old_text,new_text}`/`Terminal{terminal_id}`); `ToolCallLocation{path:PathBuf,
  line:Option<u32>}`; `PlanEntry{priority:PlanEntryPriority, status:PlanEntryStatus}` (enums); `ToolKind`/
  `ToolCallStatus` are `#[non_exhaustive]`. The bridge mapper: enums→`String` via the SDK wire spelling + a `_ =>
  "other"` arm; `locations`→`path.to_string_lossy()` (line dropped); `content_preview` per content variant
  (`Content`→capped text; `Diff`→`"path (+N/-M)"` summary, raw text DROPPED; `Terminal`→`"[terminal]"`).
  **`ToolCallUpdate` is a PRESENCE-AWARE sparse patch:** ALL fields `Option` incl. `content: Option<ContentSummary>`
  (`None` = field absent/no patch; `Some` = replace-whole, incl. empty). `ContentSummary{item_count, preview}`.
- **FIX-10 (MAJOR) — extend the `fold_journal_to_snapshot` ignore arm.** It matches `OrchEventKind` exhaustively
  with NO wildcard (`Progress | Usage => {}`, `task_store.rs:278`). Add `Plan | ToolCall | ToolCallUpdate` to that
  no-op arm (rich rows stay INERT to W3b resume). Compile-forced; make it an explicit step.
- **FIX-11 (MAJOR) — cap EVERY persisted string/vector at MAP time.** Not only tool content: `PlanEntry.content`,
  the entries vector (`RICH_VEC_CAP`, e.g. 64), `content_preview`/`locations` (`RICH_CONTENT_CAP` = 2 KiB). Caps in
  the mapper, never relying on the writer. A unit test asserts a >cap input is truncated before journaling.
- **FIX-12 (MINOR) — `record_event_sequenced` trait fallout.** Add it to the `TaskStore` trait (SQLite + Memory
  override). The test-wrapper impls (`FailingCheckpointStore` at `server.rs:8653` + `workflow_sink.rs:474`) need it
  — give the trait a DEFAULT (like `journal_fold_inputs`) that delegates to a minimal/`StoreFailure` path so wrappers
  compile unchanged; the real stores override.
- **FIX-13 (MINOR) — make the per-node (not global) ordering guarantee explicit;** `RichEventSinkFactory: Send+Sync`;
  the live frame is published INSIDE `flush` (after each durable write) so the frame's seq matches its row.

## KEYSTONE constraints (do not violate)
- **UPDATE-MINIMAL.** The backend `Update` enum (`ports.rs:19` Text/Permission/Usage/Done) gains NO variant. Rich
  events ride an INTERNAL `TurnEvent::Rich(OrchEventKind)` (bridge-acp) and a `RichEventSink` side-channel — NEVER
  the `Update` port (the text/result drain stays pure).
- **NON-BLOCKING ACP HANDLER.** `on_receive_notification` (`acp_backend.rs:963`) runs ON the SDK event loop — it
  may ONLY map + `try_send` to the per-turn mpsc (no `.await`, no store write — a blocking write there stalls EVERY
  session). The OFF-LOOP driver/sink does the async journal write.
- **FLUSH-BEFORE-NODEFINISH (ordering).** A node's queued rich events MUST be flushed (`flush().await`) and durably
  committed BEFORE its `NodeFinished`/Terminal journal row, so a node's rich rows always precede its `NodeFinished`
  in seq order. (Writer lag would otherwise commit rich rows after terminal.)
- **JOURNAL-ONLY rich rows.** Rich events write ONLY a `task_journal` row (via the shared `last_event_seq`); NO
  typed column. W3b resume (typed checkpoints) + `fold_journal_to_snapshot` (node kinds) stay INERT to rich rows.
- **NODE-FRAME byte-identity (no-rich).** The S6 node frames (node_started/node_finished/snapshot_complete/terminal)
  are UNCHANGED; the existing golden (`golden_two_node_run_wire_tuples`, a no-rich fixture) STILL passes. A run WITH
  rich events interleaves new rich frames by seq (node seqs shift) → rich runs get NEW goldens.
- **DON'T overload `TaskProgressSnapshot`.** It stays node-resume state (no observability fields). Rich transcript =
  a SEPARATE projection over `journal_from`/`journal_fold_inputs`.
- **CAP `event_json`.** Tool diffs (`old_text`/`new_text`) + `raw_input`/`raw_output` are arbitrary/large → the
  bridge DTO records a SUMMARY (ids/title/kind/status/locations + a length-capped content preview), NEVER raw
  unbounded payloads.

## Scope
**IN:**
- Bridge-owned rich `OrchEventKind` variants: **`Plan`, `ToolCall`, `ToolCallUpdate`** (Ser+De, summary-shaped,
  capped — FIX-9/11). A pure `map_session_update_rich` sibling mapper (FIX-1).
- The side-channel: a `RichEventSink` trait + a `RichEventSinkFactory` (FIX-2) in `bridge-core::ports`; a DEFAULTED
  `AgentBackend::prompt_observed`; the ACP override (rich DTO → `TurnEvent::Rich` → the unfold skip-rich loop → sink,
  FIX-5); the per-node sink built from the FACTORY in `run_node` at the NON-dispatcher prompt site (FIX-3); the
  flush barrier inside `run_node` (FIX-6).
- `TaskStore::record_event_sequenced` (journal-only sequenced writer, defaulted) on SQLite + Memory (FIX-12);
  `DetachedRichSink{store,task,op,hub,queue}` (FIX-4).
- New `FrameKind::{Plan, ToolCall, ToolCallUpdate}` + the NEW merged transcript projection fn (FIX-7) at BOTH
  reattach sites (live: raw frame per committed row; snapshot: fold — latest Plan, ToolCall folded by `tool_call_id`,
  orphan-update tolerant). Extend the `fold_journal_to_snapshot` ignore arm (FIX-10).
- DoD: a detached run's plan/tool_call events are journaled + visible in `task watch`; W3b + S6 no-rich byte-identity
  intact.

**OUT (deferred):**
- **config/mode/commands** events → a fast-follow (S7a-config); their SDK DTOs differ + they're not gate-critical
  (FIX-8).
- The **E9 watchdog** → **S7b** (this slice only lands the journal liveness it needs).
- **Warm/unary/streaming** rich journaling (no TaskStore seq substrate; `SessionManager` has no cursor) — no-op
  there until a later session-stream substrate.
- Permission DECISIONS + `awaiting_permission` state → **S9**. `session_info_update`; slash-command INVOCATION
  semantics; full UI rendering beyond JSON frames; stop-reason→`OrchResult` (P-4, no S7a consumer).

## Data model — bridge-owned rich DTOs (`crates/bridge-core/src/orch.rs`)
Mirror the ACP SDK (schema 0.13.2) shapes but with **String-valued enums (NOT raw SDK enums** — they shift under
`unstable_*` flags) and **capped content/vectors** (FIX-9/11). S7a CORE = **Plan + ToolCall + ToolCallUpdate ONLY**
(config/mode/commands DEFERRED — FIX-8). Added to `OrchEventKind` (keeping S6's NodeStarted/NodeFinished/Terminal/
Progress/Usage):
```rust
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrchEventKind {
    // … S6 variants …
    Plan { entries: Vec<PlanEntry> },                       // COMPLETE-REPLACEMENT (latest wins at projection)
    ToolCall {                                              // a new tool call (full)
        tool_call_id: String, title: String, kind: String, status: String,   // kind/status: SDK wire str, _=>"other"
        locations: Vec<String>,                            // path.to_string_lossy(), capped (RICH_VEC_CAP)
        content: Option<ContentSummary>,                   // capped; NO raw diffs/payloads
    },
    ToolCallUpdate {                                        // SPARSE PATCH keyed by tool_call_id — ALL fields Option
        tool_call_id: String,
        #[serde(skip_serializing_if="Option::is_none")] title: Option<String>,
        #[serde(skip_serializing_if="Option::is_none")] kind: Option<String>,
        #[serde(skip_serializing_if="Option::is_none")] status: Option<String>,
        #[serde(skip_serializing_if="Option::is_none")] locations: Option<Vec<String>>,   // present = replace-whole
        #[serde(skip_serializing_if="Option::is_none")] content: Option<ContentSummary>,   // None=no patch; Some=replace
    },
}
pub struct PlanEntry { pub content: String, pub priority: String, pub status: String }  // content capped; pri/status SDK str
pub struct ContentSummary { pub item_count: usize, pub preview: String }                // preview capped RICH_CONTENT_CAP
```
**Caps (FIX-11, applied at MAP time):** `RICH_CONTENT_CAP` = 2 KiB (each text preview), `RICH_VEC_CAP` = 64
(entries/locations). **`ContentSummary`** per `ToolCallContent` variant: `Content`→capped text;
`Diff{path,old_text,new_text}`→`preview = "path (+N/-M)"`, raw text DROPPED; `Terminal`→`"[terminal]"`. `raw_input`/
`raw_output` NEVER persisted. ToolCallUpdate's `content`/`locations` `Some(..)` = REPLACE-whole (ACP semantics);
`None` = no patch (presence-aware — FIX-9).

## The side-channel seam (S7a.1 — side-channel FIRST)
- **`RichEventSink` (`bridge-core::ports`):**
  ```rust
  #[async_trait] pub trait RichEventSink: Send + Sync {
      /// Non-blocking enqueue of a mapped rich event (called off the SDK loop by the driver).
      fn record(&self, kind: OrchEventKind);
      /// Await durable commit of all enqueued events for this turn (the flush barrier).
      async fn flush(&self) -> Result<(), BridgeError>;
  }
  ```
- **`AgentBackend::prompt_observed` (DEFAULTED, `ports.rs:33`):**
  ```rust
  async fn prompt_observed(&self, session: &SessionId, parts: Vec<Part>,
      _sink: Arc<dyn RichEventSink>) -> Result<BackendStream, BridgeError> {
      self.prompt(session, parts).await   // default: ignore the sink (API/Container/replay backends unchanged)
  }
  ```
  `AcpBackend` OVERRIDES it: registers the sink for the turn so the driver routes `TurnEvent::Rich` to it.
- **The ACP adapter:** extend the pure `map_session_update` (or a sibling) to map `SessionUpdate::{Plan, ToolCall,
  ToolCallUpdate, CurrentModeUpdate, ConfigOptionUpdate, AvailableCommandsUpdate}` → the rich `OrchEventKind` DTO.
  The handler (`acp_backend.rs:976`) sends `TurnEvent::Rich(kind)` to the per-session mpsc (non-blocking, exactly
  like Text/Usage). The off-loop driver, on draining a `TurnEvent::Rich`, calls `sink.record(kind)` (non-blocking
  enqueue) — it does NOT go to the `Update` unfold stream (UPDATE-MINIMAL).
- **Plumbing `{store, task, op}` to the sink (D-A / CORRECTION-1):** the DETACHED runner (`spawn_detached_workflow`,
  `server.rs:2096`, which does NOT use the dispatcher) builds a `DetachedRichSink { store, task, op: op-<task> }`
  and threads it via **`WorkflowRunContext`** (already plumbed to every node, no `run_from_with_context` signature
  change). The executor's node turn reads `ctx.rich_sink` and calls `backend.prompt_observed(session, parts, sink)`
  when present (else `prompt`). One sink per node turn (op-<task> constant; seq distinguishes).
- **The flush barrier:** the executor node turn, after the `prompt_observed` stream ends and BEFORE the node's
  result is emitted (→ before `DetachedProgressSink::node_finished` writes `NodeFinished`), calls `sink.flush().await`
  so all of THIS node's rich rows are durably committed first.

## The journal-only writer (S7a.1 — `task_store` + SQLite + Memory)
`async fn record_event_sequenced(&self, task: &TaskId, op: &OperationId, ts: i64, kind: OrchEventKind) -> Result<i64,
BridgeError>` — allocates `last_event_seq` (the SAME counter), writes ONE `task_journal` row (`insert_journal_event`
with the built `OrchEvent`), NO typed column. SQLite: one `unchecked_transaction` (bump+select+insert, like the S6
writers). Memory: under the same guard discipline as S6. `DetachedRichSink::flush` drains its queue through this
(seq-ordered). NB: with rich events enqueued during a node turn, the per-task writer is multi-producer — seq stays
atomic; ORDERING relies on flush-before-NodeFinish (KEYSTONE).

## The transcript projection (S7a.2 — projection SECOND)
- **New `FrameKind` variants** (`reattach.rs`): `Plan`, `ToolCall`, `ToolCallUpdate`, `ModeUpdate`, `ConfigUpdate`,
  `CommandsUpdate` (additive; `task_watch_cmd` is a dumb pass-through, ignores unknown kinds). A `frame_from_orch`
  helper maps each rich `OrchEventKind` → its `FrameKind` (1:1 for the LIVE tail).
- **Live tail:** `DetachedRichSink` (or the publish path) emits each COMMITTED rich row as ONE raw `WorkflowProgress
  Frame` to the hub (phase=Live), AFTER its durable write (durable-then-publish, like S6 node frames).
- **Snapshot (the fold):** a SEPARATE rich projection over `journal_from(task, cursor)` (NOT `TaskProgressSnapshot`):
  keep the LATEST `Plan` only (complete-replace); fold `ToolCall`+`ToolCallUpdate` into each `tool_call_id`'s current
  state (apply patches in seq order; tolerate an orphan update = a ToolCallUpdate with no prior ToolCall); keep the
  LATEST mode/config/commands. Emit the folded rich frames INTERLEAVED with the node frames by seq (one merged
  seq-ordered snapshot). The node-frame portion is byte-unchanged for a no-rich run.
- **Reconnect losslessness:** because the snapshot folds from durable journal rows, a reconnect after broadcast lag
  (`reattach.rs:79`) is lossless for rich events too (not broadcast-only).

## Internal sequencing (each additive, tree green)
1. **S7a.0 — rich DTOs + the ACP mapping (pure) + caps.** Add the `OrchEventKind` variants + `PlanEntry`; extend the
   pure `map_session_update`-style mapper for the 6 rich `SessionUpdate` variants → DTOs (capped); round-trip +
   mapping unit tests. No consumers wired.
2. **S7a.1 — the side-channel + journal-only writer + flush barrier.** `RichEventSink` + `prompt_observed` (defaulted
   + ACP override) + `TurnEvent::Rich` routing + `record_event_sequenced` (both stores) + `DetachedRichSink` threaded
   via `WorkflowRunContext` + flush-before-NodeFinish. Tests: a detached node's rich events journal under `op-<task>`
   in seq order, all before `NodeFinished`; non-ACP backends unaffected (default `prompt_observed`).
3. **S7a.2 — the transcript projection.** New `FrameKind`s + live raw-frame publish + the snapshot rich-fold
   (Plan-latest / ToolCall-patch / mode-config-commands-latest, interleaved by seq); the S6 no-rich golden still
   passes; a NEW rich golden locks an interleaved run.

## DoD / gate
1. **Rich events journaled + visible:** a detached workflow run whose agent emits `plan` + `tool_call`/`tool_call
   _update` → `task watch` shows `plan`/`tool_call`/`tool_call_update` frames (live + on reconnect), folded
   correctly (latest plan; tool_call current state); `event_json` is capped.
2. **Ordering + W3b:** every node's rich rows precede its `node_finished` (seq order); W3b crash-resume re-runs only
   pending nodes (rich rows inert to resume); the S6 no-rich golden byte-identical.
3. **UPDATE-MINIMAL + non-blocking:** `Update` unchanged; the ACP handler does no store write; non-ACP backends use
   the default `prompt_observed`.
4. **Gate:** `cargo fmt --all` + `cargo clippy --all-targets` clean; full workspace tests green (ci.yml floors);
   live-gate vs real codex (codex emits plan + tool_call natively) — `task watch` shows the rich transcript.
