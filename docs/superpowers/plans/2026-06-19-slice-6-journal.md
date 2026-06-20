# Slice 6 — Event-journal dual-store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Spec (BINDING, read first):** `docs/superpowers/specs/2026-06-19-slice-6-journal.md` (v3, FIX-1..16).
> **Analysis:** `docs/superpowers/specs/2026-06-19-slice-6-journal-ANALYSIS.md`. The spec's FIX list is authoritative;
> this plan is its executable decomposition. **Model roles:** codex gpt-5.5 HIGH implements; the controller (Opus)
> verifies + commits + live-gates; codex-xhigh reviews.

**Goal:** Persist a serialized `OrchEvent` journal row beside the existing typed columns in the SAME sequenced
transaction, and make `task watch` snapshot replay a byte-identical FOLD of that journal — without touching W3b
crash-resume (typed-column authoritative).

**Architecture:** Dual store — typed columns (resume axis, untouched) + a `task_journal` table (replay axis), one
shared `next_seq`, written in one transaction. Reattach folds the complete journal into a `TaskProgressSnapshot`
fed to the UNCHANGED `snapshot_frames` builder, so byte-identity is structural; eligibility-gated with a typed
fallback. The journal is replay-only; resume never reads it.

**Tech Stack:** Rust (workspace crates `bridge-core`, `bridge-store`, `bridge-a2a-inbound`, `bridge-workflow`,
`bin/a2a-bridge`), `rusqlite` (`unchecked_transaction`), `serde`/`serde_json`, `async-trait`, `tokio`.

**Conventions:** TDD (failing test → run-fail → minimal impl → run-pass → commit). `cargo fmt --all` +
`cargo clippy --all-targets` clean each commit. The `_dyld_start` codex test-exec flake recurs → the CONTROLLER
runs the suite (with a `timeout` to distinguish a real hang). Coverage floors per `.github/workflows/ci.yml`.

---

## v2 — dual plan-review fixes folded (BINDING; SUPERSEDES contradicting task text)

Dual plan-reviewed (codex-xhigh + Opus, BOTH `fix-then-implement`, fully code-grounded). These PFIXes are binding;
where a task body below contradicts a PFIX, the PFIX wins. The implementor reads this section FIRST.

- **PFIX-A (BLOCKER — codex #1) — add ONE trait method `journal_fold_inputs` for a consistent read.** Task 10's
  FIX-14/15 "one consistent read" is IMPOSSIBLE via separate `progress_snapshot()` + `journal_from()` calls on
  `Arc<dyn TaskStore>` (two lock/transaction acquisitions — a write can interleave). → add
  `async fn journal_fold_inputs(&self, task: &TaskId) -> Result<JournalFoldInputs, BridgeError>` where
  `JournalFoldInputs { complete_from_birth: bool, scalars: JournalScalars, events: Vec<OrchEvent> }`, implemented
  in SQLite under ONE `unchecked_transaction` (read the tasks row scalars + birth flag + `task_journal` rows
  together) and in Memory under one lock-consistent read. Task 10's eligibility + fold both consume THIS (no
  separate `journal_from`/`progress_snapshot`/`is_journal_complete_from_birth` calls on the wire path). `JournalScalars
  { status, result, error, terminal_seq, cut_seq }` lives in `task_store.rs` (Task 5).
- **PFIX-B (BLOCKER — both) — Task 7 updates ALL sequenced-writer call sites + BOTH `FailingCheckpointStore`s + the
  Task-1 golden, in ONE commit.** There are TWO `FailingCheckpointStore` impls: `workflow_sink.rs` test mod AND
  `crates/bridge-a2a-inbound/tests/workflow_producer.rs:2386` (the integration crate — missing it = workspace won't
  compile). Exact NEW signatures (codex #3):
  `record_node_started(task, node, operation_id: &OperationId, ts)`,
  `put_node_checkpoint_sequenced(task, node, operation_id: &OperationId, output, ok, ts)`,
  `set_terminal_sequenced(task, operation_id: &OperationId, status, result, error, ts)`.
  Call sites to update (grep `record_node_started\|put_node_checkpoint_sequenced\|set_terminal_sequenced` to
  re-confirm before editing): production `workflow_sink.rs:101/123/162`, `server.rs:2000`; the Task-1 golden;
  test doubles `workflow_sink.rs:527/549`, `workflow_producer.rs:2455/2479`; test calls `task_store.rs:704/708/719/
  735/739/747`, `sqlite.rs:1164/1201/1205/1214/1218/1223/1260/1264`, `server.rs:8580/8584/8588/8634/8638/8642/8827/
  8831/8874/8878/8912/8979/8983/9024/9029/9062`. The frozen-wire golden threads a constant `op-task-golden` (tuples
  unchanged).
- **PFIX-C (BLOCKER — both) — use REAL store APIs; the invented helpers do not exist.** There is NO
  `create_working_task`/`create_task_for_test`/`run_migrations`/`is_journal_complete_from_birth`. The create API is
  the trait `create(&self, rec: &TaskRecord)` (`task_store.rs:84`, `sqlite.rs:356`); migrations run inside
  `create_schema()` via `open`/`open_in_memory` (`sqlite.rs:33/71/136`) — idempotent intrinsically. → add a test
  helper `fn working_record(id: &str) -> TaskRecord` (model on the existing `trec()` `sqlite.rs:813` /
  `make_task_record()` `workflow_sink.rs:329`) and call `store.create(&working_record("task-…")).await`. Model the
  Task-6 idempotency test on the existing `migration_on_old_schema_db_with_cascade_and_fk` (`sqlite.rs:1067`), NOT a
  `run_migrations()` double-call.
- **PFIX-D (MAJOR — codex #4/#9) — Task 5 fold models UPSERT semantics + returns `Result`.** `record_node_started`
  is an UPSERT (`sqlite.rs:587`): a re-started in-progress node leaves only the LATEST start. The fold must REMOVE
  any existing start for a node before pushing a new `NodeStarted (node, seq)` (and on `NodeFinished`). Return
  `Result<TaskProgressSnapshot, BridgeError>` (journal node strings → `NodeId::parse` can fail on a corrupt row;
  don't unwrap/panic). Add a repeated-start invariant test (start a, start a again at a higher seq → one start).
- **PFIX-E (MAJOR — codex #5) — Memory `put_node_checkpoint_sequenced` must REJECT duplicate sequenced checkpoints
  (write-once) BEFORE seq alloc/journal write.** Today Memory overwrites (`task_store.rs:387`) where the trait says
  write-once (`:148`); without the reject, the journal and typed columns diverge and Task 7's single-checkpoint test
  misses it. Fix Memory to reject (return the same error SQLite does on the write-once violation) and add a
  both-stores divergence test (FIX-12 made concrete).
- **PFIX-F (MAJOR — both #6) — Task 1 + Task 10 goldens assert the FULL ordered `(id, seq, kind, phase)` incl.
  `SnapshotComplete` + the terminal frame + the SSE `id:` line (FIX-9).** Drive `terminal_sse_response` /
  `subscribe_to_task` (not just `snapshot_frames`) and assert the ordered tuples including the sentinel seq
  (`server.rs:1057`), the terminal seq (`server.rs:1085`), and the `id:` line (`= f.seq`, `server.rs:1095`).
- **PFIX-G (MAJOR — both #5/#7) — Task 3 EDITS the existing `OrchEvent` literal + adds a round-trip test per kind.**
  Adding `session`/`source` fields forces `session: None, source: None` into the existing literal at `orch.rs:136`
  (drop the "tests pass unchanged" wording — they pass after that mechanical edit). Round-trip Ser+De tests for ALL
  THREE journaled kinds: `NodeStarted`, `NodeFinished`, `Terminal{status,output}` (FIX-13). (No production
  `OrchEventKind::Terminal` construction exists, so adding `output` is safe.)
- **PFIX-H (MAJOR — Opus #6) — Task 10 wires the fold-or-typed choice at ALL THREE snapshot read sites** via a
  centralized helper: `server.rs:1024` (subscribe terminal branch), `:1139` (working no-hub→terminal race), `:1160`
  (post-subscribe read, whose I5 branch re-enters terminal at `:1168`). With PFIX-A, the helper does
  `journal_fold_inputs` → eligible? `fold_journal_to_snapshot(inputs)` : `progress_snapshot()`.
- **PFIX-I (MAJOR — Opus #7) — birth flag lives in the STORE, not `TaskRecord`.** SQLite: add
  `journal_complete_from_birth` to the `create` INSERT column list with literal `1` (the `NOT NULL DEFAULT 0` column
  alone yields 0 → wrongly ineligible). Memory: add a `birth: Mutex<HashSet<String>>` set in `create`, read by
  `journal_fold_inputs`. Do NOT add the bit to `TaskRecord` (store-internal eligibility marker; adding it ripples to
  every `TaskRecord` literal).
- **PFIX-J (MAJOR — Opus #8) — the migration mechanism is `migrate_tasks_columns` + `CREATE TABLE IF NOT EXISTS`,
  NOT a "versioned snapshot."** Add `("journal_complete_from_birth", "INTEGER NOT NULL DEFAULT 0")` to the
  `additive` array (`sqlite.rs:150`) and `CREATE TABLE IF NOT EXISTS task_journal(...)` to `create_schema`
  (`sqlite.rs:96`). Delete every "bump the version / versioned-snapshot" phrase (no version exists). FK
  `ON DELETE CASCADE` IS enforced (`PRAGMA foreign_keys = ON`, `sqlite.rs:27/65`).
- **PFIX-K (MINOR — both) — Task 2 test:** `SourceId::parse(...).is_ok()` (the `id_newtype!` `parse` returns
  `Result`, `ids.rs:11`), NOT `.is_some()`.
- **PFIX-L (MINOR) — placement + imports:** put `terminal_status_from_record` in `task_store.rs` (where the writers
  use it) to keep `orch`/`task_store` deps one-directional (Opus #10); Task 9's test needs `use super::*;` + import
  `WorkflowOutcome` (`bridge_workflow::executor`) and sits inside `mod sink_tests` (Opus #11). FIX-16 is ALREADY
  done (the supersede note is in the analysis doc) — no code step.

---

## File Structure

- `crates/bridge-core/src/ids.rs` — add `SessionHandleRef`, `SourceId` newtypes (FIX-11).
- `crates/bridge-core/src/orch.rs` — widen `OrchEvent`/`OrchEventKind` (FIX-1/6); status mapping; projection helper
  `orch_event_to_frame_parts`; round-trip tests (FIX-13).
- `crates/bridge-core/src/task_store.rs` — trait: add `operation_id: &OperationId` to the 3 sequenced writers, add
  `journal_from`, add `journal_complete_from_birth` to `TaskRecordStatus`/create; `MemoryTaskStore` impl (FIX-4/12);
  `fold_journal_to_snapshot` (FIX-2) lives here (operates on `TaskProgressSnapshot` + events, no I/O).
- `crates/bridge-store/src/sqlite.rs` — migration (`task_journal` + `journal_complete_from_birth`); same-tx journal
  write in the 3 writers; `journal_from`; create sets the birth flag (FIX-3/4/10).
- `crates/bridge-a2a-inbound/src/reattach.rs` — `WorkflowProgressFrame` projection helper from `OrchEvent` parts.
- `crates/bridge-a2a-inbound/src/workflow_sink.rs` — `DetachedProgressSink` passes `op-<task>` (FIX-5).
- `crates/bridge-a2a-inbound/src/server.rs` — `subscribe_to_task`/`terminal_sse_response`: eligibility → fold vs
  typed (S6.4); `finalize_detached` passes `op-<task>`.
- Test-only: `FailingCheckpointStore` (in `workflow_sink.rs` tests) gains the new writer params.

---

## Task 1 (S6.0): Freeze the wire — golden test of the current `task watch` bytes

**Files:** Test: `crates/bridge-a2a-inbound/src/server.rs` (tests mod, near the existing `subscribe_to_task` tests).

This pins byte-identity BEFORE any change. No production code.

- [ ] **Step 1: Write the golden test.** Build a `MemoryTaskStore`-backed task with a 2-node detached run via the
  existing sequenced writers (`record_node_started("a")`, `put_node_checkpoint_sequenced("a",..)`,
  `record_node_started("b")`, `put_node_checkpoint_sequenced("b",..)`, `set_terminal_sequenced(Completed,..)`), then
  call the snapshot builder (`snapshot_frames(&snap, None)` + the terminal frame block) and assert the EXACT ordered
  `Vec<(seq, kind, phase)>` plus the synthesized `SnapshotComplete` sentinel seq and the terminal frame seq.

```rust
#[tokio::test]
async fn golden_two_node_run_wire_tuples() {
    use bridge_core::task_store::{TaskStore, TaskRecordStatus};
    let store = MemoryTaskStore::new();
    let t = bridge_core::ids::TaskId::parse("task-golden").unwrap();
    store.create_task_for_test(&t).await; // helper that inserts a Working task row
    let a = bridge_core::ids::NodeId::parse("a").unwrap();
    let b = bridge_core::ids::NodeId::parse("b").unwrap();
    let s1 = store.record_node_started(&t, &a, 1).await.unwrap();
    let s2 = store.put_node_checkpoint_sequenced(&t, &a, "outA", true, 2).await.unwrap();
    let s3 = store.record_node_started(&t, &b, 3).await.unwrap();
    let s4 = store.put_node_checkpoint_sequenced(&t, &b, "outB", true, 4).await.unwrap();
    let s5 = store.set_terminal_sequenced(&t, TaskRecordStatus::Completed, Some("done"), None, 5).await.unwrap();
    assert_eq!((s1, s2, s3, s4, s5), (1, 2, 3, 4, 5));
    let snap = store.progress_snapshot(&t).await.unwrap();
    let frames = snapshot_frames(&snap, None);
    // a started+finished → only NodeFinished survives; b same. Ordered by seq.
    let tuples: Vec<(i64, &str)> = frames.iter().map(|f| (f.seq, frame_kind_tag(&f.kind))).collect();
    assert_eq!(tuples, vec![(2, "node_finished"), (4, "node_finished")]);
    // sentinel seq = last snapshot frame seq (4); terminal seq = terminal_seq (5).
    assert_eq!(snap.terminal_seq, Some(5));
    assert_eq!(snap.cut_seq, 5);
}
```
(Add the tiny `frame_kind_tag(&FrameKind) -> &str` + `create_task_for_test` test helpers if absent.)

- [ ] **Step 2: Run — expect PASS** (it characterizes current behavior).
  Run: `cargo test -p bridge-a2a-inbound golden_two_node_run_wire_tuples -- --nocapture`. Expected: PASS.
- [ ] **Step 3: Commit.** `git add -A && git commit -m "test(slice-6): S6.0 golden — freeze task watch wire tuples"`

---

## Task 2 (S6.1): Add `SessionHandleRef` / `SourceId` newtypes

**Files:** Modify `crates/bridge-core/src/ids.rs`. Test: same file.

- [ ] **Step 1: Failing test.**
```rust
#[test]
fn session_and_source_newtypes_roundtrip() {
    let s = SessionHandleRef::parse("h-1").unwrap();
    let j = serde_json::to_value(&s).unwrap();
    assert_eq!(j, serde_json::json!("h-1"));
    assert_eq!(serde_json::from_value::<SessionHandleRef>(j).unwrap(), s);
    assert!(SourceId::parse("src-1").is_some());
}
```
- [ ] **Step 2: Run — expect FAIL** (types undefined). `cargo test -p bridge-core session_and_source_newtypes_roundtrip`
- [ ] **Step 3: Implement.** Add next to the existing macro invocations: `id_newtype!(SessionHandleRef);
  id_newtype!(SourceId);` (mirror an existing `id_newtype!` usage exactly — same derives/`parse`).
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit.** `git commit -am "feat(core): SessionHandleRef + SourceId id newtypes (slice-6 FIX-11)"`

---

## Task 3 (S6.1): Widen `OrchEvent` / `OrchEventKind` + status mapping

**Files:** Modify `crates/bridge-core/src/orch.rs`. Test: same file.

- [ ] **Step 1: Failing tests** — new variants round-trip; back-compat preserved; status mapping total.
```rust
#[test]
fn node_started_finished_terminal_roundtrip() {
    let ev = OrchEvent {
        v: ORCH_V, seq: 4, ts_ms: 9, operation_id: OperationId::parse("op-t1").unwrap(),
        session: None, source: None,
        kind: OrchEventKind::NodeFinished { node: "a".into(), ok: true, output: "o".into() },
    };
    let j = serde_json::to_value(&ev).unwrap();
    assert_eq!(j["kind"], "node_finished");
    assert_eq!(j["node"], "a");
    assert!(j.get("session").is_none() && j.get("source").is_none()); // skip_serializing_if
    assert_eq!(serde_json::from_value::<OrchEvent>(j).unwrap().seq, 4);
}
#[test]
fn task_record_status_maps_total() {
    use bridge_core::task_store::TaskRecordStatus as S;
    assert!(matches!(terminal_status_from_record(&S::Completed), TerminalStatus::Completed));
    assert!(matches!(terminal_status_from_record(&S::Canceled), TerminalStatus::Canceled));
    for s in [S::Failed, S::Interrupted, S::Working] {
        assert!(matches!(terminal_status_from_record(&s), TerminalStatus::Failed { .. }));
    }
}
```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p bridge-core orch::`
- [ ] **Step 3: Implement.** Add the `session`/`source` envelope fields (with `#[serde(skip_serializing_if =
  "Option::is_none", default)]`) to `OrchEvent`; add `NodeStarted`/`NodeFinished`/`Terminal { status, output }` to
  `OrchEventKind` (KEEP `Progress`/`Usage`). Add:
```rust
pub fn terminal_status_from_record(s: &crate::task_store::TaskRecordStatus) -> TerminalStatus {
    use crate::task_store::TaskRecordStatus as S;
    match s {
        S::Completed => TerminalStatus::Completed,
        S::Canceled => TerminalStatus::Canceled,
        other => TerminalStatus::Failed { reason: other.as_str().to_string() }, // "failed"/"interrupted"/"working"
    }
}
```
  (Import `TaskRecordStatus`; if it lacks `as_str`, use its existing string repr.) Confirm the EXISTING orch.rs
  tests (`orch_event_roundtrips_with_internal_kind_tag`, etc.) still pass unchanged (back-compat, FIX-7).
- [ ] **Step 4: Run — expect PASS** (new + all existing orch tests).
- [ ] **Step 5: Commit.** `git commit -am "feat(core): widen OrchEvent kinds + total status map (slice-6 FIX-1/6)"`

---

## Task 4 (S6.1): `OrchEvent` → `WorkflowProgressFrame` projection

**Files:** Modify `crates/bridge-a2a-inbound/src/reattach.rs` (projection helper + golden). Test: same file.

The projection produces the frame parts; `phase` is supplied by the caller (Snapshot for snapshot rows, Live for
the tail) since `OrchEvent` has no phase (FIX-2).

- [ ] **Step 1: Failing test** — each journaled kind projects to the exact frozen `FrameKind`.
```rust
#[test]
fn orch_event_projects_to_frozen_frame() {
    use bridge_core::orch::{OrchEvent, OrchEventKind, TerminalStatus, ORCH_V};
    let mk = |kind| OrchEvent { v: ORCH_V, seq: 7, ts_ms: 0,
        operation_id: bridge_core::ids::OperationId::parse("op-x").unwrap(),
        session: None, source: None, kind };
    let f = frame_from_orch(&mk(OrchEventKind::NodeFinished { node: "a".into(), ok: false, output: "o".into() }),
                            Phase::Snapshot);
    assert!(matches!(f.kind, FrameKind::NodeFinished { ok: false, .. }) && f.seq == 7);
    let ft = frame_from_orch(&mk(OrchEventKind::Terminal {
        status: TerminalStatus::Failed { reason: "interrupted".into() }, output: "boom".into() }), Phase::Live);
    assert!(matches!(ft.kind, FrameKind::Terminal { outcome: TerminalOutcome::Failed, .. }));
}
```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p bridge-a2a-inbound orch_event_projects_to_frozen_frame`
- [ ] **Step 3: Implement** `pub(crate) fn frame_from_orch(ev: &OrchEvent, phase: Phase) -> WorkflowProgressFrame`:
  map `NodeStarted→FrameKind::NodeStarted{node}`, `NodeFinished→{node,ok,output}`, `Terminal{status,output}→
  {outcome: outcome_from_status(&status), output}`; `outcome_from_status`: Completed→Completed, Canceled→Canceled,
  `Failed{..}→Failed`. (Progress/Usage are not journaled → `unreachable!`/`debug_assert` or omit.)
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit.** `git commit -am "feat(inbound): OrchEvent→WorkflowProgressFrame projection (slice-6 FIX-2)"`

---

## Task 5 (S6.1): `fold_journal_to_snapshot` + the frame-level invariant

**Files:** Modify `crates/bridge-core/src/task_store.rs` (pure fold fn). Test: same file (+ a frame-level invariant
test in `reattach.rs`/`server.rs` tests in Task 10).

`fold_journal_to_snapshot(events, scalars) -> TaskProgressSnapshot` replays the FOLD: a node's `NodeStarted` adds a
start; its `NodeFinished` removes that start + adds a checkpoint; `Terminal` clears all starts. Scalars
(`status`, `result`/`error`, `terminal_seq`, `cut_seq`) come from the tasks row (FIX-15) — for the journaled path
`result = Some(terminal.output)` (so the frame builder's `result.or(error)` reproduces the wire output).

- [ ] **Step 1: Failing test** — the fold reproduces a hand-built typed snapshot's CHECKPOINTS+STARTS.
```rust
#[test]
fn fold_collapses_started_then_finished() {
    use bridge_core::orch::*; use bridge_core::ids::*;
    let op = OperationId::parse("op-t").unwrap();
    let ev = |seq, kind| OrchEvent { v: ORCH_V, seq, ts_ms: 0, operation_id: op.clone(),
        session: None, source: None, kind };
    let events = vec![
        ev(1, OrchEventKind::NodeStarted { node: "a".into() }),
        ev(2, OrchEventKind::NodeFinished { node: "a".into(), ok: true, output: "oA".into() }),
        ev(3, OrchEventKind::NodeStarted { node: "b".into() }), // b still in progress
    ];
    let scalars = JournalScalars { status: TaskRecordStatus::Working, result: None, error: None,
        terminal_seq: None, cut_seq: 3 };
    let snap = fold_journal_to_snapshot(&events, &scalars);
    assert_eq!(snap.checkpoints.iter().map(|c| (c.0.as_str().to_string(), c.3)).collect::<Vec<_>>(),
               vec![("a".to_string(), 2)]);
    assert_eq!(snap.starts.iter().map(|s| (s.0.as_str().to_string(), s.1)).collect::<Vec<_>>(),
               vec![("b".to_string(), 3)]);
    assert_eq!(snap.cut_seq, 3);
}
```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p bridge-core fold_collapses_started_then_finished`
- [ ] **Step 3: Implement** `JournalScalars { status, result, error, terminal_seq, cut_seq }` + `fold_journal_to
  _snapshot`: iterate events in seq order maintaining a `Vec<(NodeId, seq)>` starts + `Vec<(NodeId,String,bool,i64)>`
  checkpoints; on `NodeStarted` push a start; on `NodeFinished` retain-drop the matching start + push a checkpoint;
  on `Terminal` clear starts (and, for the journaled path, set `result = Some(output)` if the caller didn't already
  supply scalars from the row). Return `TaskProgressSnapshot { status, result, error, checkpoints, starts,
  terminal_seq, cut_seq }`.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit.** `git commit -am "feat(core): fold_journal_to_snapshot (slice-6 FIX-2/15)"`

---

## Task 6 (S6.2): Migration — `task_journal` table + `journal_complete_from_birth`

**Files:** Modify `crates/bridge-store/src/sqlite.rs` (schema/migration). Test: same file.

- [ ] **Step 1: Failing test** — a fresh DB has the table + column; migration is idempotent (run twice).
```rust
#[tokio::test]
async fn migration_adds_journal_table_and_birth_flag() {
    let store = SqliteStore::open_in_memory().unwrap();
    store.run_migrations().unwrap();
    store.run_migrations().unwrap(); // idempotent
    let conn = store.conn.lock().unwrap();
    let cnt: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='task_journal'", [], |r| r.get(0)).unwrap();
    assert_eq!(cnt, 1);
    let has_col: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info('tasks') WHERE name='journal_complete_from_birth'", [], |r| r.get(0)).unwrap();
    assert_eq!(has_col, 1);
}
```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p bridge-store migration_adds_journal_table_and_birth_flag`
- [ ] **Step 3: Implement.** In the schema/migration list (alongside the existing additive column adds + the W3a/W3b
  versioned snapshot), add: `CREATE TABLE IF NOT EXISTS task_journal(task_id TEXT NOT NULL, seq INTEGER NOT NULL,
  event_json TEXT NOT NULL, PRIMARY KEY(task_id, seq), FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE)`
  and `ALTER TABLE tasks ADD COLUMN journal_complete_from_birth INTEGER NOT NULL DEFAULT 0` guarded the same way the
  existing additive columns are (check-pragma-then-add, idempotent). Bump the snapshot/migration version if the
  pattern requires it.
- [ ] **Step 4: Run — expect PASS** (+ the existing migration tests, incl. the legacy-NULL-seq test `:1161`).
- [ ] **Step 5: Commit.** `git commit -am "feat(store): task_journal table + journal_complete_from_birth (slice-6 FIX-3/10)"`

---

## Task 7 (S6.2): Same-tx journal write + `journal_from` + birth flag at create

**Files:** Modify `crates/bridge-core/src/task_store.rs` (trait sigs + `MemoryTaskStore`), `crates/bridge-store/src/
sqlite.rs` (impl), test-only `FailingCheckpointStore` (in `workflow_sink.rs` tests). Tests: both store crates.

This is the keystone (FIX-4). The 3 sequenced writers gain `operation_id: &OperationId`, build+serialize the
`OrchEvent` internally, and INSERT the journal row in the SAME transaction; on first journal write set
`journal_complete_from_birth` only if the task was created under S6 (Task 8 sets it at create — here writers do NOT
set it). Add `journal_from`.

- [ ] **Step 1: Failing test** (SQLite + Memory) — after a sequenced write, `journal_from` returns the event with
  the column seq; the typed columns are unchanged.
```rust
async fn journal_write_matches_typed<S: TaskStore>(store: S) {
    let t = TaskId::parse("task-j").unwrap();
    store.create_task_for_test(&t).await;
    let a = NodeId::parse("a").unwrap();
    let op = OperationId::parse("op-task-j").unwrap();
    let s1 = store.record_node_started(&t, &a, &op, 1).await.unwrap();
    let s2 = store.put_node_checkpoint_sequenced(&t, &a, &op, "oA", true, 2).await.unwrap();
    let evs = store.journal_from(&t, -1).await.unwrap();
    assert_eq!(evs.len(), 2);
    assert!(matches!(evs[0].kind, OrchEventKind::NodeStarted { .. }) && evs[0].seq == s1);
    assert!(matches!(&evs[1].kind, OrchEventKind::NodeFinished { output, .. } if output == "oA") && evs[1].seq == s2);
    // re-stamped seq == column seq; operation_id is op-<task>
    assert_eq!(evs[0].operation_id.as_str(), "op-task-j");
}
#[tokio::test] async fn sqlite_journal_write() { journal_write_matches_typed(SqliteStore::open_in_memory().unwrap()).await }
#[tokio::test] async fn memory_journal_write() { journal_write_matches_typed(MemoryTaskStore::new()).await }
```
- [ ] **Step 2: Run — expect FAIL** (sig mismatch + `journal_from` missing). `cargo test -p bridge-store -p bridge-core journal`
- [ ] **Step 3: Implement.**
  - Trait: add `operation_id: &OperationId` param to `record_node_started`/`put_node_checkpoint_sequenced`/
    `set_terminal_sequenced`; add `async fn journal_from(&self, task: &TaskId, after_seq: i64) ->
    Result<Vec<OrchEvent>, BridgeError>`.
  - SQLite: inside each writer's existing `unchecked_transaction`, AFTER allocating `seq` + the typed write, build
    the `OrchEvent { v: ORCH_V, seq, ts_ms: ts, operation_id: operation_id.clone(), session: None, source: None,
    kind }` (kind from the typed args; Terminal uses `terminal_status_from_record(status)` + `output =
    result.or(error).unwrap_or_default()`), `serde_json::to_string`, and `INSERT INTO task_journal(task_id, seq,
    event_json) VALUES(?,?,?)`. `journal_from`: `SELECT seq, event_json FROM task_journal WHERE task_id=?1 AND
    seq>?2 ORDER BY seq`, deserialize, re-stamp `seq` from the column.
  - Memory: same, under the single-writer discipline (write the journal `Vec` entry within the same method, before
    returning `seq`, no `.await` between alloc + insert — FIX-12).
  - `FailingCheckpointStore` + any other test impl: add the new params (delegate or no-op the journal).
- [ ] **Step 4: Run — expect PASS** (both stores; + existing store tests).
- [ ] **Step 5: Commit.** `git commit -am "feat(store): same-tx journal write + journal_from, both stores (slice-6 FIX-4/12)"`

---

## Task 8 (S6.2): Set `journal_complete_from_birth` at task create

**Files:** Modify `crates/bridge-store/src/sqlite.rs` + `crates/bridge-core/src/task_store.rs` (Memory) create paths.
Test: both.

- [ ] **Step 1: Failing test** — a task created under S6 code has the flag = 1; a legacy row (inserted without it)
  defaults to 0.
```rust
#[tokio::test]
async fn create_sets_birth_flag() {
    let store = SqliteStore::open_in_memory().unwrap();
    let t = TaskId::parse("task-b").unwrap();
    store.create_working_task(&t /* existing create signature */).await.unwrap();
    assert!(store.is_journal_complete_from_birth(&t).await.unwrap());
}
```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p bridge-store create_sets_birth_flag`
- [ ] **Step 3: Implement.** In the existing task-create/insert path, set `journal_complete_from_birth = 1`. Add a
  small read `is_journal_complete_from_birth(&TaskId) -> bool` (SELECT the column; default 0). Memory mirror.
- [ ] **Step 4: Run — expect PASS.**
- [ ] **Step 5: Commit.** `git commit -am "feat(store): birth-flag at create (slice-6 FIX-3)"`

---

## Task 9 (S6.3): Detached path passes `op-<task>`; journals each event

**Files:** Modify `crates/bridge-a2a-inbound/src/workflow_sink.rs` (`DetachedProgressSink`) +
`crates/bridge-a2a-inbound/src/server.rs` (`finalize_detached`). Test: `workflow_sink.rs` tests.

The sink/finalizer already hold the stable `task`; build `op-<task>` and pass it to the writers (which now journal).
KEEP publishing the existing `WorkflowProgressFrame` to the hub (unchanged).

- [ ] **Step 1: Failing test** — driving `DetachedProgressSink` over a 1-node stream writes journal rows under
  `op-<task>` AND publishes the same frames as before.
```rust
#[tokio::test]
async fn detached_sink_journals_under_op_task() {
    let store = Arc::new(MemoryTaskStore::new());
    let t = TaskId::parse("task-d").unwrap(); store.create_working_task(&t).await.unwrap();
    let hub = Arc::new(TaskProgressHub::new());
    let mut sink = DetachedProgressSink::new(store.clone(), t.clone(), hub);
    sink.node_started("a").await.unwrap();
    sink.node_finished("a", true, "oA").await.unwrap();
    sink.terminal(WorkflowOutcome::Completed, "done".into()).await.unwrap();
    let evs = store.journal_from(&t, -1).await.unwrap();
    assert!(evs.iter().all(|e| e.operation_id.as_str() == "op-task-d"));
    assert!(matches!(evs.last().unwrap().kind, OrchEventKind::Terminal { .. }));
}
```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p bridge-a2a-inbound detached_sink_journals_under_op_task`
- [ ] **Step 3: Implement.** In the sink, compute `let op = OperationId::parse(&format!("op-{}", self.task.as_str()))
  .expect("valid op id")` once (or store it in `new`), and pass `&op` to each sequenced writer call. Mirror in
  `finalize_detached` (its `set_terminal_sequenced` call). No other behavior changes (still publishes the frame).
- [ ] **Step 4: Run — expect PASS** (+ existing detached/W3b tests unchanged).
- [ ] **Step 5: Commit.** `git commit -am "feat(inbound): detached path journals under op-<task> (slice-6 FIX-5)"`

---

## Task 10 (S6.4): Reattach folds the journal (eligible) with typed fallback

**Files:** Modify `crates/bridge-a2a-inbound/src/server.rs` (`subscribe_to_task` + `terminal_sse_response` snapshot
build). Test: same file.

Snapshot source becomes: if ELIGIBLE (`journal_complete_from_birth=1` AND (non-terminal OR `terminal_seq IS NOT
NULL`)) → `fold_journal_to_snapshot(journal_from(t,-1), scalars-from-row)` → the UNCHANGED `snapshot_frames(snap,
cursor)` + terminal frame + sentinel; ELSE → today's typed `progress_snapshot()`. The `cursor` is applied by
`snapshot_frames` (FIX-15). Both byte-identical (the Task-1 golden + the invariant below).

- [ ] **Step 1: Failing tests** — (a) a born-under-S6 fully-journaled task yields frames identical to the typed
  path; (b) a cancel_if_working-terminated task (terminal_seq NULL) falls back to typed; (c) a non-birth task falls
  back.
```rust
#[tokio::test]
async fn eligible_task_folds_journal_byte_identical() {
    // build a 2-node completed task via the sequenced writers (birth flag set at create)
    // frames_from_fold == frames_from_typed (the FIX-2 frame-level invariant)
    let typed = snapshot_frames(&store.progress_snapshot(&t).await.unwrap(), None);
    let folded = snapshot_frames(&fold_snapshot_for(&store, &t).await.unwrap(), None);
    assert_eq!(serialize_all(&typed), serialize_all(&folded));
}
#[tokio::test]
async fn cancel_if_working_task_uses_typed_fallback() {
    // create + record_node_started + cancel_if_working → terminal_seq NULL → eligibility false
    assert!(!is_journal_fold_eligible(&store, &t).await.unwrap());
}
```
- [ ] **Step 2: Run — expect FAIL.** `cargo test -p bridge-a2a-inbound eligible_task_folds_journal_byte_identical`
- [ ] **Step 3: Implement.** Add `is_journal_fold_eligible(store, task) -> bool` (reads the birth flag +
  `progress_snapshot` status/terminal_seq) and `fold_snapshot_for(store, task) -> TaskProgressSnapshot` (reads
  `journal_from(t,-1)` + builds `JournalScalars` from the tasks row, calls `fold_journal_to_snapshot`). In
  `subscribe_to_task`/`terminal_sse_response`, choose the snapshot source by eligibility; feed it to the SAME
  `snapshot_frames(&snap, cursor)` + terminal-frame + `SnapshotComplete` code (unchanged). Read the journal + the
  scalars at ONE consistent point (FIX-14: the sentinel/`cut` seq must equal the snapshot's `cut_seq`).
- [ ] **Step 4: Run — expect PASS** (+ the existing subscribe/reattach/Last-Event-ID tests unchanged).
- [ ] **Step 5: Commit.** `git commit -am "feat(inbound): reattach folds journal w/ typed fallback (slice-6 FIX-2/3/14/15)"`

---

## Task 11: Gate + per-increment review prep

- [ ] **Step 1: Full gate (controller runs).** `cargo fmt --all --check` ; `cargo clippy --all-targets
  --all-features -- -D warnings` ; `cargo test --workspace --all-targets` (use a `timeout` per the `_dyld_start`
  flake note). All green; coverage floors per `.github/workflows/ci.yml`.
- [ ] **Step 2: Whole-branch review.** Per the project discipline (the Slice-5 lesson: whole-`main...HEAD`
  codex-xhigh review iterate-to-clean catches cross-task bugs the per-task reviews miss), run the whole-branch
  review BEFORE merge; iterate to APPROVE.
- [ ] **Step 3: Live-gate.** `serve` a detached workflow → disconnect mid-run → `task watch <id> --from <seq>`
  replays byte-identical ordered frames; reconnect with `Last-Event-ID` is exactly-once; crash mid-run resumes only
  pending nodes (W3b intact). Vs real codex+claude.
- [ ] **Step 4: Merge** to `main` once the whole-branch review is clean (controller commits).

---

## Self-Review (controller, against the spec)

- **Spec coverage:** S6.0 (Task 1) · S6.1 DTOs+projection+fold (Tasks 2–5) · S6.2 store (Tasks 6–8) · S6.3 detached
  journaling (Task 9) · S6.4 reattach fold (Task 10) · gate/live-gate (Task 11). FIX-1..16 each map to a task step.
- **Type consistency:** `terminal_status_from_record` (Task 3) used by the SQLite Terminal write (Task 7) +
  projection `outcome_from_status` (Task 4); `JournalScalars`/`fold_journal_to_snapshot` (Task 5) used by Task 10;
  the writer signature gains `operation_id: &OperationId` consistently across trait + both impls + test stores
  (Task 7) and the callers (Task 9).
- **Risk note:** Task 7 is the breaking-signature keystone (every `TaskStore` impl + `FailingCheckpointStore`); the
  plan-review must confirm no caller is missed (grep the 3 method names workspace-wide before implementing).
