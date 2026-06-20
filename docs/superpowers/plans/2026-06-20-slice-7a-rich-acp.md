# Slice 7a — Rich ACP event journaling + transcript — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (or executing-plans).
> **Spec (BINDING, read first incl. the `## v2 … FIX-1..13` section):** `docs/superpowers/specs/2026-06-20-slice-7a-
> rich-acp.md`. **Analysis:** `…/2026-06-20-slice-7-rich-acp-ANALYSIS.md`. **Model roles:** codex-HIGH implements;
> controller (Opus) verifies + commits + live-gates; codex-xhigh reviews.

**Goal:** capture the ACP `plan`/`tool_call`/`tool_call_update` notifications the bridge drops, map to bridge-owned
`OrchEventKind` DTOs, journal them on the Slice-6 substrate for a DETACHED run, and surface them in `task watch` —
without touching `Update` (UPDATE-MINIMAL), without blocking the SDK loop, and with rich rows committed before each
node's `NodeFinished` (the flush barrier).

**Architecture:** rich `session/update` → a pure sibling mapper → an internal `TurnEvent::Rich` → the off-loop
unfold's skip-rich loop → a dependency-inverted `RichEventSink` (built per-node from a factory carried in
`WorkflowRunContext`) → `record_event_sequenced` (journal-only, shared `last_event_seq`). Reattach gains new
`FrameKind`s + a merged seq-ordered projection (node frames + folded rich). Detached-only.

**Tech stack:** Rust workspace (bridge-core, bridge-acp, bridge-store, bridge-workflow, bridge-a2a-inbound),
agent-client-protocol 0.12.1 (schema 0.13.2), serde, async-trait, tokio. TDD; fmt+clippy clean each commit;
controller runs the suite (the `_dyld_start` codex flake); coverage floors per `.github/workflows/ci.yml`.

---

## v2 — dual plan-review fixes folded (BINDING; SUPERSEDES contradicting task text)
Dual plan-reviewed (codex-xhigh + Opus, BOTH `fix-then-implement`; architecture sound, executable details have
compile-blockers). Read PFIX-A..K FIRST.
- **PFIX-A (BLOCKER — both) — the factory `make(&NodeId)` has NO `op` param; it CLOSES OVER `op`.** `run_node` has
  no `OperationId`/`TaskId` in scope (`executor.rs:99`; `WorkflowRunContext` carries only `session_cwd`,
  `executor.rs:20`). → `trait RichEventSinkFactory { fn make(&self, node: &NodeId) -> Arc<dyn RichEventSink>; }`;
  `DetachedRichSinkFactory { store, task, op, hub }` is built in `spawn_detached_workflow` (`server.rs:2055`), where
  `op = OperationId::parse(format!("op-{}", task))` exactly as `DetachedProgressSink::operation_id()` does
  (`workflow_sink.rs:90`). `run_node` calls `factory.make(&node.id)`.
- **PFIX-B (BLOCKER — Opus) — `ToolCallUpdate` fields are NESTED.** SDK `ToolCallUpdate { tool_call_id,
  #[serde(flatten)] fields: ToolCallUpdateFields, meta }` (`schema/v1/tool_call.rs:169`); `ToolCallUpdateFields`
  (`:217`) holds `Option<{kind,status,title,content,locations,raw_input,raw_output}}`. The mapper reads
  `u.tool_call_id` + `u.fields.{kind,status,title,content,locations}` (NOT `u.status`).
- **PFIX-C (BLOCKER — Opus) — the SDK types are `#[non_exhaustive]`.** `SessionUpdate`/`Plan`/`PlanEntry`/
  `PlanEntryPriority`/`PlanEntryStatus`/`ToolCall`/`ToolCallUpdate(Fields)`/`ToolKind`/`ToolCallStatus`/
  `ToolCallContent`/`ToolCallLocation`/`ContentBlock` are all non_exhaustive → NO downstream struct literals; the
  mapper's `match notif.update` needs a `_ => None` arm, every enum→str needs `_ => "other"`. Test fixtures BUILD SDK
  values via constructors (`Plan::new(vec![])`, `ToolCall::new(id,title)`+builders, `ContentChunk::new`,
  `TextContent::new`) — mirror the EXISTING test pattern at `acp_backend.rs:2994-3022`. (Bridge-owned
  `OrchEventKind`/`PlanEntry`/`ContentSummary` ARE struct-literal-constructible.)
- **PFIX-D (BLOCKER — both) — `ContentChunk` indirection + NO `Display` on SDK enums.** `AgentMessageChunk(ContentChunk)`
  → `chunk.content` is a `ContentBlock` (`acp_backend.rs:1633`); the rich `Content` arm goes `tcc.content` →
  `ContentBlock::Text(t) => &t.text`. The SDK enums derive `Serialize` but NOT `Display` → hand-write per-variant
  `match → &'static str`. Wire spellings: priority `high|medium|low`; plan status `pending|in_progress|completed`;
  `ToolKind` `read|edit|delete|move|search|execute|think|fetch|switch_mode|other`; `ToolCallStatus`
  `pending|in_progress|completed|failed`.
- **PFIX-E (BLOCKER — both) — REORDER: FrameKinds + `frame_from_orch` (Task 7) BEFORE the `DetachedRichSink` publish
  (Task 6).** Task 6's `flush` calls `frame_from_orch` + the new `FrameKind`s; if Task 7 lands later, T6 doesn't
  compile. → swap: do the current Task 7 as Task 6, the current Task 6 as Task 7 (or merge the frame additions into
  the sink task). `FrameKind`/`frame_from_orch` are `pub(crate)` (cross-module: `workflow_sink.rs` uses
  `reattach::frame_from_orch`).
- **PFIX-F (BLOCKER — codex+Opus) — flush on ALL exit paths + handle the `Err` (NO `.ok()`).** The barrier must hold
  on the cancel-before-prompt / prompt-error / mid-drain-cancel early returns (`executor.rs:218/225/231/240`), not
  just the happy path (`:257`). A silently-swallowed `flush` `Err` lets `NodeFinished` (`executor.rs:471`) commit
  with rich rows missing. → on the happy path, `sink.flush().await?`-style: on `Err`, return a node FAILURE
  (`("[node {} rich-flush failed: …]", false)`); on the early-return paths, flush (best-effort or fail) before the
  `return`. Use a guard or an explicit flush at each site. Confirm `run_node` return precedes the outer
  `yield NodeFinished` (it does: return `:258` → yield `:471`).
- **PFIX-G (BLOCKER — codex) — `DetachedRichSink.queue` is `std::sync::Mutex`, NOT tokio.** `record` is SYNC
  (`&self`), but `workflow_sink.rs:72` imports `tokio::sync::Mutex` → an unqualified `Mutex<VecDeque<_>>` can't lock
  from `record`. Use `std::sync::Mutex<VecDeque<OrchEventKind>>` (fully-qualified); `flush` drains into a local
  `Vec` BEFORE any `.await`.
- **PFIX-H (MAJOR — both) — the projection needs ONE consistent read + a node+events input; it can't just swap
  `snapshot_frames`.** `fold_or_typed_snapshot` (`server.rs:1102`) returns a node-only `TaskProgressSnapshot` and
  DISCARDS `journal_fold_inputs().events`; `terminal_sse_response`/`working_sse` take `&TaskProgressSnapshot`
  (`server.rs:1049/1194`). → change `fold_or_typed_snapshot` (or a sibling) to return `{ snap: TaskProgressSnapshot,
  events: Vec<OrchEvent> }` from the ONE `journal_fold_inputs` read; add `rich_snapshot_frames(snap: &TaskProgress
  Snapshot, events: &[OrchEvent], cursor) -> Vec<WorkflowProgressFrame>` = `snapshot_frames(snap, cursor)` (node
  frames, byte-identical) MERGED seq-sorted with the folded rich frames; thread it into BOTH sites preserving the
  `SnapshotComplete` sentinel (re-derived from the MERGED last frame) + the working-SSE `dedup_floor` (`server.rs:1205`).
  For a no-rich task `events` has no rich kinds → identical node frames → the S6 golden holds.
- **PFIX-I (MAJOR — codex) — presence-aware content summary.** `ToolCallUpdateFields.content: Option<Vec<ToolCallContent>>`:
  `Some(vec![])` = replace-with-empty → `Some(ContentSummary{item_count:0, preview:""})`, `None` = no patch. Full
  `ToolCall.content` (always present) → `Some(ContentSummary{item_count: original_len, preview})` (or `None` only if
  truly empty — pick one and test it). `item_count` = the ORIGINAL vector length (before cap). Per `ToolCallContent`:
  `Content`→text from `ContentBlock::Text` (else a `[non-text]` placeholder), `Diff`→`"path (+N/-M)"`, `Terminal`→
  `"[terminal]"`.
- **PFIX-J (MAJOR — codex) — DTO derives.** `OrchEventKind` derives `Clone, Debug, Serialize, Deserialize` → add the
  SAME to `PlanEntry` + `ContentSummary`: `#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]`.
- **PFIX-K (MINOR) — (a)** handler order is RICH-FIRST (borrow `&notif`) THEN the value-consuming `map_session_update`
  — this AVOIDS cloning the by-value notif (the spec's "existing-first" would force a clone); rich variants are
  disjoint from text/usage so it's equivalent. **(b)** the sink is captured in the `prompt_inner` UNFOLD, NOT
  "registered for the turn" in the connection handler (correct the spec wording). **(c)** the `record_event
  _sequenced` default protects the 2 custom `TaskStore` impls: `LegacyFallbackStore` (`server.rs:8653`) +
  `FailingCheckpointStore` (`tests/workflow_producer.rs:2373`). **(d)** `frame_from_orch`: `content_preview =
  content.as_ref().map(|c| c.preview.clone())`.

---

## File Structure
- `crates/bridge-core/src/orch.rs` — Plan/ToolCall/ToolCallUpdate `OrchEventKind` variants + PlanEntry/ContentSummary.
- `crates/bridge-core/src/task_store.rs` — extend the `fold_journal_to_snapshot` ignore arm; add
  `record_event_sequenced` (trait default + Memory); `RichEventSink`/`RichEventSinkFactory` (or in ports.rs).
- `crates/bridge-core/src/ports.rs` — `RichEventSink`/`RichEventSinkFactory` traits; defaulted `prompt_observed`.
- `crates/bridge-acp/src/acp_backend.rs` — `map_session_update_rich`; `TurnEvent::Rich`; `prompt_observed`/
  `prompt_inner(rich_sink)`; the unfold skip-rich loop; the handler dual-map.
- `crates/bridge-store/src/sqlite.rs` — `record_event_sequenced` (one-tx journal-only).
- `crates/bridge-workflow/src/executor.rs` — `WorkflowRunContext.make_rich_sink`; build per-node sink + call
  `prompt_observed` at the non-dispatcher site (`:229`); flush before return (`:257`).
- `crates/bridge-a2a-inbound/src/workflow_sink.rs` — `DetachedRichSink{store,task,op,hub,queue}` + a
  `RichEventSinkFactory` impl.
- `crates/bridge-a2a-inbound/src/server.rs` — build the factory + inject into `ctx` in `spawn_detached_workflow`
  (`:2096`); the merged rich projection fn + swap at both reattach sites (`:1054`,`:1194`).
- `crates/bridge-a2a-inbound/src/reattach.rs` — `FrameKind::{Plan,ToolCall,ToolCallUpdate}` + `frame_from_orch`.

---

## Task 1 (S7a.0): Rich `OrchEventKind` DTOs + extend the resume-fold ignore arm

**Files:** `crates/bridge-core/src/orch.rs`; `crates/bridge-core/src/task_store.rs` (the fold arm). Tests: both.

- [ ] **Step 1: Failing tests** — round-trip the 3 kinds + the presence-aware ToolCallUpdate.
```rust
#[test]
fn rich_kinds_roundtrip() {
    let tc = OrchEventKind::ToolCall { tool_call_id: "t1".into(), title: "read".into(),
        kind: "read".into(), status: "in_progress".into(), locations: vec!["a.rs".into()],
        content: Some(ContentSummary { item_count: 1, preview: "hello".into() }) };
    let j = serde_json::to_value(&OrchEvent{ v:ORCH_V, seq:5, ts_ms:1,
        operation_id: OperationId::parse("op-t").unwrap(), session:None, source:None, kind: tc }).unwrap();
    assert_eq!(j["kind"], "tool_call"); assert_eq!(j["tool_call_id"], "t1");
    let _: OrchEvent = serde_json::from_value(j).unwrap();
    // ToolCallUpdate: absent fields are omitted (sparse patch)
    let up = serde_json::to_value(&OrchEventKind::ToolCallUpdate{ tool_call_id:"t1".into(),
        title:None, kind:None, status:Some("completed".into()), locations:None, content:None }).unwrap();
    assert_eq!(up["kind"], "tool_call_update"); assert_eq!(up["status"], "completed");
    assert!(up.get("title").is_none() && up.get("content").is_none());
}
```
- [ ] **Step 2: Run → FAIL.** `cargo test -p bridge-core rich_kinds_roundtrip`
- [ ] **Step 3: Implement.** Add to `OrchEventKind` (snake_case tag): `Plan { entries: Vec<PlanEntry> }`, `ToolCall
  { tool_call_id, title, kind, status, locations: Vec<String>, content: Option<ContentSummary> }`, `ToolCallUpdate {
  tool_call_id, #[serde(skip_serializing_if="Option::is_none")] title: Option<String>, … kind, status, locations:
  Option<Vec<String>>, content: Option<ContentSummary> }`. Add `pub struct PlanEntry { content: String, priority:
  String, status: String }` and `pub struct ContentSummary { item_count: usize, preview: String }` (both Ser+De).
  Then EXTEND `fold_journal_to_snapshot`'s ignore arm (`task_store.rs:278`): `Progress | Usage | Plan | ToolCall |
  ToolCallUpdate => {}` (rich rows inert to resume — FIX-10).
- [ ] **Step 4: Run → PASS** (+ existing orch + fold tests). `cargo test -p bridge-core orch:: fold_`; `cargo test
  --workspace --no-run`.
- [ ] **Step 5: Commit.** `git commit -am "feat(core): rich OrchEventKind DTOs + resume-fold inert arm (s7a FIX-9/10)"`

---

## Task 2 (S7a.0): The pure `map_session_update_rich` sibling mapper + caps

**Files:** `crates/bridge-acp/src/acp_backend.rs`. Tests: same file.

- [ ] **Step 1: Failing tests** — SDK Plan/ToolCall/ToolCallUpdate → capped DTOs; non-rich → None.
```rust
#[test]
fn map_rich_plan_and_toolcall() {
    use agent_client_protocol::{SessionUpdate, SessionNotification /*+ Plan/ToolCall ctors*/};
    let plan_notif = /* SessionNotification with SessionUpdate::Plan(Plan{entries:[..]}) */;
    matches!(AcpBackend::map_session_update_rich(&plan_notif), Some(OrchEventKind::Plan{..}));
    let tc_notif = /* SessionUpdate::ToolCall(ToolCall::new("t1","read")) */;
    let Some(OrchEventKind::ToolCall{ tool_call_id, .. }) = AcpBackend::map_session_update_rich(&tc_notif)
        else { panic!() };
    assert_eq!(tool_call_id, "t1");
    // a Text chunk maps to None on the rich path
    assert!(AcpBackend::map_session_update_rich(&text_notif).is_none());
}
#[test]
fn map_rich_caps_content() {
    let big = "x".repeat(10_000);
    let notif = /* ToolCall with a Content block of `big` */;
    let Some(OrchEventKind::ToolCall{ content: Some(cs), .. }) = AcpBackend::map_session_update_rich(&notif) else { panic!() };
    assert!(cs.preview.len() <= RICH_CONTENT_CAP);
}
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** `pub fn map_session_update_rich(notif: &SessionNotification) -> Option<OrchEventKind>`
  matching `SessionUpdate::{Plan, ToolCall, ToolCallUpdate}` (the other variants → `None`). Map enum→String via the
  SDK wire spelling with a `_ => "other".to_string()` arm (ToolKind/ToolCallStatus are `#[non_exhaustive]`);
  `locations` → `loc.path.to_string_lossy().into_owned()` capped to `RICH_VEC_CAP`; `content` → `ContentSummary`
  per `ToolCallContent` variant (Content→capped text; Diff→`format!("{} (+{}/-{})", path, plus, minus)`, raw text
  DROPPED; Terminal→`"[terminal]"`). Add `const RICH_CONTENT_CAP: usize = 2048; const RICH_VEC_CAP: usize = 64;` +
  a `cap(s: &str)` helper (char-boundary safe). Plan entries: `priority`/`status` via SDK spelling; `content` capped;
  entries capped to `RICH_VEC_CAP`.
- [ ] **Step 4: Run → PASS.** `cargo test -p bridge-acp map_rich`; `cargo clippy -p bridge-acp --all-targets`.
- [ ] **Step 5: Commit.** `git commit -am "feat(acp): map_session_update_rich sibling mapper + caps (s7a FIX-1/9/11)"`

---

## Task 3 (S7a.1): `RichEventSink`/`RichEventSinkFactory` + defaulted `prompt_observed`

**Files:** `crates/bridge-core/src/ports.rs`. Tests: a trivial sink in `ports.rs` tests.

- [ ] **Step 1: Failing test** — the default `prompt_observed` delegates to `prompt` (a backend that doesn't
  override it ignores the sink).
```rust
#[tokio::test]
async fn prompt_observed_defaults_to_prompt() {
    // a stub AgentBackend whose `prompt` yields one Text+Done; prompt_observed (default) must behave identically
    let b = StubBackend::new();
    let sink: Arc<dyn RichEventSink> = Arc::new(CountingSink::default());
    let s = b.prompt_observed(&sid, vec![], sink.clone()).await.unwrap();
    // drains to the same Update sequence; sink.record never called (default ignores it)
    assert_eq!(sink_count(&sink), 0);
}
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** in `ports.rs`:
```rust
#[async_trait::async_trait]
pub trait RichEventSink: Send + Sync {
    fn record(&self, kind: crate::orch::OrchEventKind);          // sync non-blocking enqueue
    async fn flush(&self) -> Result<(), BridgeError>;            // await durable commit (the barrier)
}
pub trait RichEventSinkFactory: Send + Sync {
    fn make(&self, node: &NodeId, op: &OperationId) -> std::sync::Arc<dyn RichEventSink>;
}
```
  Add a DEFAULTED trait method to `AgentBackend`:
```rust
async fn prompt_observed(&self, session: &SessionId, parts: Vec<Part>,
    _sink: std::sync::Arc<dyn RichEventSink>) -> Result<BackendStream, BridgeError> {
    self.prompt(session, parts).await
}
```
- [ ] **Step 4: Run → PASS** (+ `cargo test --workspace --no-run` — the defaulted method keeps every backend impl
  compiling).
- [ ] **Step 5: Commit.** `git commit -am "feat(core): RichEventSink/Factory + defaulted prompt_observed (s7a FIX-2)"`

---

## Task 4 (S7a.1): `record_event_sequenced` (journal-only) — SQLite + Memory + trait default

**Files:** `crates/bridge-core/src/task_store.rs` (trait default + Memory), `crates/bridge-store/src/sqlite.rs`.
Tests: both stores.

- [ ] **Step 1: Failing test** (both stores) — a rich event journals under op + a fresh seq, NO typed column.
```rust
async fn rich_event_journals<S: TaskStore>(store: S) {
    let t = TaskId::parse("task-r").unwrap(); store.create(&working_record("task-r")).await.unwrap();
    let op = OperationId::parse("op-task-r").unwrap();
    let seq = store.record_event_sequenced(&t, &op, 7,
        OrchEventKind::Plan{ entries: vec![] }).await.unwrap();
    let evs = store.journal_from(&t, -1).await.unwrap();
    assert_eq!(evs.len(), 1);
    assert!(matches!(evs[0].kind, OrchEventKind::Plan{..}) && evs[0].seq == seq);
    // no typed checkpoint/start written (rich is journal-only)
    let snap = store.progress_snapshot(&t).await.unwrap();
    assert!(snap.checkpoints.is_empty() && snap.starts.is_empty());
}
#[tokio::test] async fn sqlite_rich(){ rich_event_journals(SqliteStore::open_in_memory().unwrap()).await }
#[tokio::test] async fn memory_rich(){ rich_event_journals(MemoryTaskStore::new()).await }
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement.** Trait: `async fn record_event_sequenced(&self, task: &TaskId, op: &OperationId, ts:
  i64, kind: OrchEventKind) -> Result<i64, BridgeError>` with a DEFAULT returning `Err(BridgeError::StoreFailure)`
  (so the test-wrapper `FailingCheckpointStore`s compile; the real stores override). SQLite: one
  `unchecked_transaction` — bump+select `last_event_seq`, build `OrchEvent{ v:ORCH_V, seq, ts_ms:ts, operation_id:
  op.clone(), session:None, source:None, kind }`, `insert_journal_event(&tx, task, &ev)`, commit. NO typed write.
  Memory: under the journal guard (S6 discipline) — alloc seq, push the journal row, NO typed map write.
- [ ] **Step 4: Run → PASS** (both stores; + `cargo test --workspace --no-run`).
- [ ] **Step 5: Commit.** `git commit -am "feat(store): record_event_sequenced journal-only writer (s7a FIX-12)"`

---

## Task 5 (S7a.1): ACP `prompt_observed` override + `TurnEvent::Rich` + the unfold skip-rich loop

**Files:** `crates/bridge-acp/src/acp_backend.rs`. Tests: same file.

- [ ] **Step 1: Failing test** — a rich `session/update` during a turn routes to the sink (`record`), NOT the
  `Update` stream; Text/Usage still flow as `Update`.
```rust
#[tokio::test]
async fn prompt_observed_routes_rich_to_sink() {
    // drive a fake turn whose handler receives a ToolCall update + a Text chunk + Done.
    let sink = Arc::new(CountingSink::default());
    let stream = backend.prompt_observed(&session, vec![], sink.clone()).await.unwrap();
    let updates: Vec<_> = stream.collect().await; // only Text + Done — NO rich
    assert!(updates.iter().all(|u| !matches!(u, Ok(Update::Permission(_)))));
    assert_eq!(sink.records(), 1); // the ToolCall went to the sink
}
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement.**
  - Add `Rich(bridge_core::orch::OrchEventKind)` to `TurnEvent` (`acp_backend.rs:211`).
  - The handler (`acp_backend.rs:976`): try the rich mapper FIRST (borrow), else the value mapper:
    `if let Some(kind) = Self::map_session_update_rich(&notif) { Some(TurnEvent::Rich(kind)) } else { match Self::
    map_session_update(notif) { Some(Update::Text(t)) => Some(TurnEvent::Text(t)), Some(Update::Usage(s)) =>
    Some(TurnEvent::Usage(s)), _ => None } }` → `tx.send(te)`.
  - Refactor `prompt` → `prompt_inner(&self, session, parts, rich_sink: Option<Arc<dyn RichEventSink>>)`; `prompt`
    = `prompt_inner(.., None)`; override `prompt_observed` = `prompt_inner(.., Some(sink))`.
  - The unfold (`:1859`): capture `rich_sink` into the state `(rx, done, sink)`; wrap `rx.recv()` in a `loop`:
    `Some(TurnEvent::Rich(k)) => { if let Some(s) = &sink { s.record(k); } continue }`; Text/Usage/Done/Failed as
    today; `None => return None`. (Rich NEVER yields an `Update` — UPDATE-MINIMAL.)
- [ ] **Step 4: Run → PASS** (+ the existing acp turn/corpus tests unchanged; `cargo test -p bridge-acp`; clippy).
- [ ] **Step 5: Commit.** `git commit -am "feat(acp): prompt_observed + TurnEvent::Rich + unfold skip-rich loop (s7a FIX-1/5)"`

---

## Task 6 (S7a.1): `DetachedRichSink` + factory + executor wiring + the flush barrier

**Files:** `crates/bridge-a2a-inbound/src/workflow_sink.rs` (sink+factory), `crates/bridge-workflow/src/executor.rs`
(ctx field + per-node sink + prompt_observed at `:229` + flush at `:257`), `crates/bridge-a2a-inbound/src/server.rs`
(build factory + inject into ctx at `:2096`). Tests: workflow_sink.rs + an executor test.

- [ ] **Step 1: Failing test** — a detached node's rich events journal under `op-<task>` in seq order, ALL before
  `NodeFinished`; and a non-rich detached run is unchanged.
```rust
#[tokio::test]
async fn detached_node_journals_rich_before_nodefinished() {
    // a 1-node detached run on a fake backend that emits ToolCall(seqs)+Text then Done;
    // assert journal order: NodeStarted < ToolCall < NodeFinished (the flush barrier).
    let evs = store.journal_from(&task, -1).await.unwrap();
    let kinds: Vec<&str> = evs.iter().map(|e| kind_tag(&e.kind)).collect();
    assert_eq!(kinds, vec!["node_started", "tool_call", "node_finished"]);
}
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement.**
  - `WorkflowRunContext` (`executor.rs:20`): add `pub make_rich_sink: Option<Arc<dyn RichEventSinkFactory>>`
    (Default None; Clone via Arc; scheduler MUST NOT read it).
  - `run_node` NON-dispatcher branch (`executor.rs:229`): if `ctx.make_rich_sink` is Some, `let sink =
    factory.make(&node.id, &op_for_this_run); let s = resolved.backend.prompt_observed(&session, vec![Part{text:
    rendered}], sink.clone()).await` (else `prompt`). After the drain loop, BEFORE `forget_session`+`return`
    (`:257`): `if let Some(sink)=&sink { sink.flush().await.ok(); }` (flush = durable-commit + publish this node's
    rich rows). (`op_for_this_run` = the detached `op-<task>`; thread it the same way the cwd/ctx is — via ctx or
    the factory closure capturing it.)
  - `DetachedRichSink { store: Arc<dyn TaskStore>, task: TaskId, op: OperationId, hub: Arc<TaskProgressHub>, queue:
    Mutex<VecDeque<OrchEventKind>> }`: `record` = push to `queue` (sync, non-blocking); `flush` = drain the queue,
    for each `record_event_sequenced(&store, &task, &op, now_ms(), kind)` → `seq` → publish
    `WorkflowProgressFrame{seq, phase:Live, kind: frame_from_orch(kind)}` to `hub` (durable-then-publish). A
    `DetachedRichSinkFactory { store, task, hub }` impl of `RichEventSinkFactory::make(node, op)` returns a fresh
    `DetachedRichSink`.
  - `spawn_detached_workflow` (`server.rs:2096`): build the factory `Arc::new(DetachedRichSinkFactory{ store:
    srv.task_store.clone(), task: task.clone(), hub: hub.clone() })` and set `ctx.make_rich_sink = Some(factory)`
    BEFORE `run_from_with_context(.., ctx)`.
- [ ] **Step 4: Run → PASS** (+ existing executor/detached/W3b tests unchanged; non-ACP backends use the default
  `prompt_observed`; `cargo test --workspace --no-run`).
- [ ] **Step 5: Commit.** `git commit -am "feat(inbound): DetachedRichSink + factory + executor wiring + flush (s7a FIX-2/3/4/6)"`

---

## Task 7 (S7a.2): New `FrameKind`s + live publish

**Files:** `crates/bridge-a2a-inbound/src/reattach.rs`. (Live publish already wired in Task 6's flush.) Tests: same.

- [ ] **Step 1: Failing test** — `frame_from_orch` maps each rich kind to its frame.
```rust
#[test]
fn frame_from_orch_rich() {
    let f = frame_from_orch(&OrchEventKind::ToolCall{ tool_call_id:"t1".into(), title:"x".into(),
        kind:"read".into(), status:"completed".into(), locations:vec![], content:None }, Phase::Live, 5);
    assert!(matches!(f.kind, FrameKind::ToolCall{..}) && f.seq == 5);
}
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement.** Add `FrameKind::{Plan{entries}, ToolCall{tool_call_id,title,kind,status,locations,
  content_preview:Option<String>}, ToolCallUpdate{tool_call_id, …Option}}` (serialize-only, snake_case, flattened).
  Add `pub(crate) fn frame_from_orch(kind: &OrchEventKind, phase: Phase, seq: i64) -> WorkflowProgressFrame` for the
  rich kinds (node kinds aren't routed here). Wire it in Task-6's `DetachedRichSink::flush` publish (already
  referenced).
- [ ] **Step 4: Run → PASS.**
- [ ] **Step 5: Commit.** `git commit -am "feat(inbound): rich FrameKinds + frame_from_orch (s7a FIX-7)"`

---

## Task 8 (S7a.2): The merged seq-ordered snapshot projection + both reattach sites + goldens

**Files:** `crates/bridge-a2a-inbound/src/server.rs`. Tests: same (the S6 golden + a new rich golden).

- [ ] **Step 1: Failing tests** — (a) the S6 no-rich golden (`golden_two_node_run_wire_tuples`) STILL passes; (b) a
  new rich golden: a journaled run with NodeStarted→ToolCall→ToolCallUpdate→NodeFinished folds to node_started? (no
  — node started+finished collapses) + ONE folded `tool_call` (merged state) + terminal, seq-ordered.
```rust
#[tokio::test]
async fn rich_snapshot_folds_toolcall_and_interleaves() {
    // journal: node_started(1) tool_call(2,t1,in_progress) tool_call_update(3,t1->completed) node_finished(4)
    // snapshot frames (no cursor): node_finished@4 (start collapsed) + tool_call@3 (folded current=completed),
    // ordered by seq, then SnapshotComplete, then terminal.
    let frames = rich_snapshot_frames(&fold_inputs, None);
    assert_eq!(tags(&frames), vec![("tool_call", 3 /*last-applied seq*/), ("node_finished", 4)]);
}
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** `fn rich_snapshot_frames(events: &[OrchEvent], cursor: Option<i64>) -> Vec<Workflow
  ProgressFrame>`: fold the events ONCE — node kinds → the S6 collapse (reuse `fold_journal_to_snapshot` for the
  node portion OR inline); rich kinds → keep the LATEST `Plan`; per `tool_call_id` fold `ToolCall`+`ToolCallUpdate`
  into the current state, emitting ONE `ToolCall` frame at the LAST-applied seq if a base existed, else a
  `ToolCallUpdate` frame at the update seq (orphan). Merge all into ONE seq-sorted `Vec<(seq,FrameKind)>`,
  cursor-filter (`seq>K`). Replace the `snapshot_frames(&snap, cursor)` call at BOTH reattach sites
  (`server.rs:1054` + `:1194`) with the merged projection (reading `journal_fold_inputs(task).events`); keep the
  `SnapshotComplete` sentinel seq + `dedup_floor`/`cut_seq` logic. (For an ELIGIBLE no-rich task this yields the
  identical node frames → the S6 golden holds.)
- [ ] **Step 4: Run → PASS** (the S6 golden + the new rich golden + the existing reattach/Last-Event-ID tests;
  `cargo test -p bridge-a2a-inbound`).
- [ ] **Step 5: Commit.** `git commit -am "feat(inbound): merged rich snapshot projection, both sites (s7a FIX-7)"`

---

## Task 9: Gate + whole-branch review + live-gate + merge (controller)

- [ ] **Step 1: Full gate** (controller): `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --
  -D warnings`; `cargo test --workspace --exclude bridge-container` (timeout-guarded for the `_dyld_start` flake);
  coverage floors per ci.yml.
- [ ] **Step 2: Whole-branch review** — codex-xhigh + Opus on the whole `main...HEAD` diff (the cross-task net),
  iterate to clean. Focus: the unfold loop preserving UPDATE-MINIMAL + termination; the flush barrier ordering
  under concurrent siblings; the seq interleaving vs the S6 node byte-identity; the projection orphan-update
  handling; cap correctness; the non-ACP default path.
- [ ] **Step 3: Live-gate** vs real codex (codex emits `plan` + `tool_call` natively): a detached `serve` run →
  `task watch` shows `plan`/`tool_call`/`tool_call_update` frames (live + on reconnect, folded); the S6 node frames
  intact; W3b crash-resume still completes; `event_json` capped.
- [ ] **Step 4: Merge** to `main` once the whole-branch review is clean (controller commits).

---

## Self-Review (controller, against the spec)
- **FIX coverage:** FIX-1 (T2/T5 sibling mapper) · FIX-2 (T3/T6 factory) · FIX-3 (T6 `:229`) · FIX-4 (T6
  hub+publish) · FIX-5 (T5 unfold loop) · FIX-6 (T6 flush `:257`) · FIX-7 (T7/T8 projection) · FIX-8 (scope: 3
  variants only) · FIX-9 (T1/T2 precise DTOs) · FIX-10 (T1 fold arm) · FIX-11 (T2 caps) · FIX-12 (T4 default) ·
  FIX-13 (T6 per-node flush + publish-in-flush).
- **Type consistency:** `RichEventSink`/`RichEventSinkFactory` (T3) used by `DetachedRichSink` (T6) + the executor
  (T6); `OrchEventKind` rich variants (T1) used by the mapper (T2), the writer (T4), the projection (T8);
  `frame_from_orch` (T7) used by T6's publish + T8.
- **Risk:** T5 (the unfold loop + prompt_inner refactor) is the trickiest — the plan-review must confirm the unfold
  termination + that Rich never reaches the `Update` consumer; T6's flush-before-return ordering + the `op`
  threading to the per-node sink need a close look.
