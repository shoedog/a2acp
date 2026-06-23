# Slice 10 — B2: Weighted Fan-out Panel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the existing workflow fan-out→synth pattern into a **weighted panel** — N agents independently analyze the same input; a synth node compares them across pros/cons/usage/benefit/risk with **operator-configured weights** and emits a weighted recommendation in markdown, surfaced with REAL per-source usage that survives crash-resume.

**Architecture:** Markdown-first (ADR-0012; no global JSON), reusing the workflow DAG executor (NOT `fanout.rs`). The one real code primitive is **per-node usage capture threaded durably through the fold→seed→`run_from` resume chain**; the panel itself is then a synth contract (prompt + config) that consumes two new reserved synth template vars — `{{workflow.costs}}` (computed from captured per-node usage) and `{{workflow.weights}}` (from `[workflows.panel]` config).

**Tech Stack:** Rust (workspace crates: bridge-core, bridge-store, bridge-coordinator, bridge-workflow, bridge-a2a-inbound, bin/a2a-bridge), tokio, serde, rusqlite, async-trait.

**Binding spec:** `docs/superpowers/specs/2026-06-22-slice-10-fanout-panel.md` — the `## v2` section (SF-FIX-1..6). Base = `main` `4d8c66d`. Branch `feat/slice-10-fanout-panel`.

---

## File Structure (what each task touches)

| File | Responsibility | Tasks |
|---|---|---|
| `crates/bridge-core/src/orch.rs` | `OrchEventKind::NodeFinished` durable journal event — add `usage` | T1 |
| `crates/bridge-core/src/task_store.rs` | `TaskStore` trait sigs (`put_node_checkpoint_sequenced`, `node_checkpoints`), `fold_journal_to_snapshot`, InMemory store | T1, T2 |
| `crates/bridge-store/src/sqlite.rs` | `usage_json` column + migration + read/write + journal event | T2 |
| `crates/bridge-coordinator/src/detached.rs` | `WorkflowSink` trait + `drain_workflow` + `DetachedProgressSink` + `FrameKind::NodeFinished` + resume seed | T3, T6 |
| `crates/bridge-workflow/src/executor.rs` | `WorkflowEvent::NodeFinished.usage`, per-node usage capture, fan-in `{{workflow.costs}}`/`{{workflow.weights}}` injection, `run_from` seed type | T3, T4, T5, T6 |
| `crates/bridge-workflow/src/graph.rs` | `WorkflowGraph.panel` + `PanelConfig` | T5 |
| `crates/bridge-a2a-inbound/src/server.rs` | `SseSink::node_finished` 4th param (ignores usage) | T3 |
| `bin/a2a-bridge/src/main.rs` | `run-workflow` CLI printer match arm | T3 |
| `bin/a2a-bridge/src/config.rs` | `[workflows.panel]` TOML parse → `PanelConfig` | T5 |
| `examples/a2a-bridge.panel.toml`, `prompts/panel-member.md`, `prompts/panel-synth.md` | the `panel` workflow instance | T7 |

**Task order is bottom-up so the tree stays green after every task:** durable event field (T1) → durable store (T2) → wire/sink plumbing (T3) → executor capture + costs injection (T4) → weights config + graph field (T5) → resume carries usage (T6) → panel workflow (T7) → degrade + watch surfacing (T8).

## Reference facts (verified against the code — do not re-derive)

- `UsageSnapshot` (`bridge-core/src/orch.rs:37`): `{ used: Option<u64>, size: Option<u64>, cost: Option<UsageCost>, at_ms: i64 }`. `UsageCost { amount: f64, currency: String }`. `windowFraction = used/size`. Already `Clone + Debug + PartialEq + Default + Serialize + Deserialize`.
- `Update::Usage(crate::orch::UsageSnapshot)` (`bridge-core/src/ports.rs:24`) — the variant `run_node` currently ignores at `executor.rs:169` (warm/dispatcher path) and `:275` (cold path).
- `OrchEventKind::NodeFinished { node, ok, output }` (`orch.rs:66`) is the durable journal event; `WorkflowEvent::NodeFinished { node, ok, output }` (`executor.rs:80`) is the in-process executor event; `FrameKind::NodeFinished { node, ok, output }` (`detached.rs:69`) is the SSE/`task watch` wire frame. **Three distinct types** — all three gain `usage`.
- The `WorkflowSink` trait lives ONCE in `detached.rs:182` (re-exported via `bridge-a2a-inbound/src/workflow_sink.rs`). Impls of `node_finished`: `DetachedProgressSink` (`detached.rs:301`) and `SseSink` (`server.rs:1850`), plus the no-op default and test fakes.
- `NodeId::parse` charset is `[a-z0-9_-]+` (`ids.rs`), so `workflow.costs`/`workflow.weights` (with `.`) can never be a node id → reserved synth-var namespace is collision-proof (SF-FIX-3). The template renderer (`template.rs:8`) matches the literal token between `{{`/`}}` against the vars map, so dotted keys render fine.
- The durable snapshot is `encode_workflow_spec(graph)` → `{"v":1,"graph":<WorkflowGraph>}` (`detached.rs:1322`), round-tripped by `WorkflowSpecEnvelope { v, graph }` (`detached.rs:1310`) in `resume_working_tasks` (`detached.rs:1345`). Because the panel config will live ON `WorkflowGraph` with `#[serde(default, skip_serializing_if=Option::is_none)]`, weights serialize into the snapshot automatically (additive-safe) and survive resume with **zero** changes to the envelope/resume code.
- The resume seed is built at `detached.rs:1423` from `task_store.node_checkpoints(&task)`. The seed type is `HashMap<String, (String, bool)>` (`executor.rs:373`, `detached.rs:1115`).
- `WorkflowGraph` has **25 struct-literal construction sites** (`grep -rn "WorkflowGraph {"`). Adding a field forces `panel: None` at each — mechanical; the file list is in T5.

---

## Task 1: Add `usage` to the durable `NodeFinished` journal event

**Files:**
- Modify: `crates/bridge-core/src/orch.rs:66` (the `OrchEventKind::NodeFinished` variant + its roundtrip tests)
- Modify: `crates/bridge-core/src/task_store.rs:284` (`fold_journal_to_snapshot` match arm)
- Modify: `crates/bridge-store/src/sqlite.rs:684` (journal event construction in `put_node_checkpoint_sequenced`)
- Modify: `crates/bridge-coordinator/src/detached.rs:991` (the `OrchEventKind::NodeFinished { .. } => "node_finished"` mapping — already uses `..`, so no change needed; verify)

- [ ] **Step 1: Write the failing test** — add to `orch.rs` tests (after the existing `journaled_orch_event_kinds_roundtrip`):

```rust
#[test]
fn node_finished_carries_optional_usage() {
    // usage present → serializes; absent → field omitted (skip_serializing_if).
    let with_usage = OrchEventKind::NodeFinished {
        node: "a".into(),
        ok: true,
        output: "o".into(),
        usage: Some(UsageSnapshot {
            used: Some(15071),
            size: Some(258400),
            cost: None,
            at_ms: 5,
        }),
    };
    let j = serde_json::to_value(&with_usage).unwrap();
    assert_eq!(j["kind"], "node_finished");
    assert_eq!(j["usage"]["used"], 15071);

    let without = OrchEventKind::NodeFinished {
        node: "a".into(),
        ok: true,
        output: "o".into(),
        usage: None,
    };
    let j2 = serde_json::to_value(&without).unwrap();
    assert!(j2.get("usage").is_none(), "absent usage omitted from wire");

    // Old rows on the wire (no `usage` key) must still deserialize (default None).
    let old: OrchEventKind =
        serde_json::from_value(serde_json::json!({"kind":"node_finished","node":"a","ok":true,"output":"o"})).unwrap();
    assert!(matches!(old, OrchEventKind::NodeFinished { usage: None, .. }));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-core node_finished_carries_optional_usage`
Expected: FAIL — compile error (`NodeFinished` has no field `usage`).

- [ ] **Step 3: Add the field** — in `orch.rs:66`, change the variant to:

```rust
    NodeFinished {
        node: String,
        ok: bool,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<UsageSnapshot>,
    },
```

Then fix the now-broken destructure sites (the compiler lists them):
- `task_store.rs:284` — change the arm to `OrchEventKind::NodeFinished { node, ok, output, usage: _ } => { ... }` (fold ignores usage — snapshot-replay surfacing is a tracked deferral; checkpoints stay a 4-tuple).
- `sqlite.rs:684` — add `usage: None,` to the journal-event literal **for now** (T2 replaces it with the real value).
- The existing `orch.rs` test `journaled_orch_event_kinds_roundtrip` (`orch.rs:236`) — add `usage: None,` to its `NodeFinished` literal.
- Any other `OrchEventKind::NodeFinished {` literal the compiler flags — add `usage: None`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p bridge-core orch::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/orch.rs crates/bridge-core/src/task_store.rs crates/bridge-store/src/sqlite.rs
git commit -m "feat(orch): add optional usage to NodeFinished journal event"
```

---

## Task 2: Persist + read per-node usage in the TaskStore (column + trait)

**Files:**
- Modify: `crates/bridge-core/src/task_store.rs` — trait sigs `put_node_checkpoint_sequenced` (`:166`) + `node_checkpoints` (`:130`); InMemory store (`CheckpointValue` `:317`, the impls `:456`/`:535`)
- Modify: `crates/bridge-store/src/sqlite.rs` — migration (`:173`), `put_node_checkpoint_sequenced` (`:634`), `node_checkpoints` (`:506`)
- Modify: `crates/bridge-a2a-inbound/tests/workflow_producer.rs:2462` (test-fake store impl of `put_node_checkpoint_sequenced`/`node_checkpoints`)

**Design:** `node_checkpoints` returns `Vec<(NodeId, String, bool, Option<UsageSnapshot>)>` (was a 3-tuple). `put_node_checkpoint_sequenced` gains `usage: Option<&UsageSnapshot>`, persists it to a new nullable `usage_json` column AND into the journal `NodeFinished{usage}` (replacing T1's `usage: None` placeholder). `progress_snapshot`/`TaskProgressSnapshot.checkpoints` are UNTOUCHED (4-tuple) — the resume seed reads `node_checkpoints` directly, not the snapshot.

- [ ] **Step 1: Write the failing test** — add to `bridge-store/src/sqlite.rs` tests:

```rust
#[tokio::test]
async fn node_checkpoint_roundtrips_usage_and_old_rows_read_none() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = TaskId::parse("t-usage").unwrap();
    let op = OperationId::parse("op-t-usage").unwrap();
    store.create(&sample_task(&task)).await.unwrap(); // existing helper that inserts a Working row
    let node = NodeId::parse("member").unwrap();
    let usage = bridge_core::orch::UsageSnapshot { used: Some(15071), size: Some(258400), cost: None, at_ms: 7 };
    store
        .put_node_checkpoint_sequenced(&task, &node, &op, "OUT", true, 7, Some(&usage))
        .await
        .unwrap();

    let cps = store.node_checkpoints(&task).await.unwrap();
    assert_eq!(cps.len(), 1);
    let (n, out, ok, got) = &cps[0];
    assert_eq!(n.as_str(), "member");
    assert_eq!(out, "OUT");
    assert!(ok);
    assert_eq!(got.as_ref().unwrap().used, Some(15071));

    // A legacy checkpoint row with usage_json NULL → None.
    let node2 = NodeId::parse("legacy").unwrap();
    store
        .put_node_checkpoint_sequenced(&task, &node2, &op, "L", true, 8, None)
        .await
        .unwrap();
    let cps = store.node_checkpoints(&task).await.unwrap();
    let legacy = cps.iter().find(|(n, ..)| n.as_str() == "legacy").unwrap();
    assert!(legacy.3.is_none(), "absent usage reads back as None");
}
```

(Use whatever existing test helper inserts a Working task row; if none, inline a `TaskRecord` `create`. Confirm `SqliteStore::open_in_memory` is the test constructor used elsewhere in this file.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-store node_checkpoint_roundtrips_usage`
Expected: FAIL — `put_node_checkpoint_sequenced` takes 6 args, not 7.

- [ ] **Step 3: Implement**

(a) Migration — `sqlite.rs:173`, in the `task_node_checkpoints` block of `migrate_tasks_columns`, after the `seq` ALTER:

```rust
    if !cp_existing.contains("usage_json") {
        conn.execute_batch("ALTER TABLE task_node_checkpoints ADD COLUMN usage_json TEXT;")?;
    }
```

(b) Trait — `task_store.rs:166`, add the param:

```rust
    async fn put_node_checkpoint_sequenced(
        &self,
        task: &TaskId,
        node: &NodeId,
        operation_id: &OperationId,
        output: &str,
        ok: bool,
        ts: i64,
        usage: Option<&crate::orch::UsageSnapshot>,
    ) -> Result<i64, BridgeError>;
```

And `node_checkpoints` (`:130`):

```rust
    async fn node_checkpoints(
        &self,
        task: &TaskId,
    ) -> Result<Vec<(NodeId, String, bool, Option<crate::orch::UsageSnapshot>)>, BridgeError>;
```

(c) Sqlite write — `sqlite.rs:634`. Serialize usage and store in both the column and the journal event:

```rust
        let usage_json = usage.map(|u| serde_json::to_string(u)).transpose().map_err(|_| BridgeError::StoreFailure)?;
        // ... seq allocation unchanged ...
        tx.execute(
            "INSERT INTO task_node_checkpoints(task_id, node_id, output, ok, ts, seq, usage_json)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![task.as_str(), node.as_str(), output, ok as i64, ts, seq, usage_json],
        ).map_err(|_| BridgeError::StoreFailure)?;
        // ... DELETE start row unchanged ...
        kind: bridge_core::orch::OrchEventKind::NodeFinished {
            node: node.as_str().to_string(),
            ok,
            output: output.to_string(),
            usage: usage.cloned(),
        },
```

(d) Sqlite read — `sqlite.rs:506` `node_checkpoints`, change the query + row mapping:

```rust
        .prepare("SELECT node_id, output, ok, usage_json FROM task_node_checkpoints WHERE task_id=?1")
        // ... in the loop, after ok_i:
        let usage_s: Option<String> = row.get(3).map_err(|_| BridgeError::StoreFailure)?;
        let usage = usage_s
            .map(|s| serde_json::from_str::<bridge_core::orch::UsageSnapshot>(&s))
            .transpose()
            .map_err(|_| BridgeError::StoreFailure)?;
        out.push((node, output, ok_i != 0, usage));
```

(e) InMemory store — `task_store.rs`. Change `CheckpointValue` (`:317`) from `(String, bool, i64, i64)` to `(String, bool, i64, i64, Option<crate::orch::UsageSnapshot>)`; update `put_node_checkpoint_sequenced` (`:535`) to store the usage and `node_checkpoints` (`:456`) to return it; the `put_node_checkpoint` (non-sequenced, `:433`) path stores `None`. The `progress_snapshot` builder (`:760`) keeps its existing 4-tuple — drop the new usage element there.

- [ ] **Step 4: Run tests**

Run: `cargo test -p bridge-store node_checkpoint_roundtrips_usage && cargo test -p bridge-core task_store`
Expected: PASS. (The `workflow_producer.rs` fake store will fail to compile until its impl is updated — do that now: add the `usage` param to its `put_node_checkpoint_sequenced` and the 4th tuple element to `node_checkpoints`.)

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/task_store.rs crates/bridge-store/src/sqlite.rs crates/bridge-a2a-inbound/tests/workflow_producer.rs
git commit -m "feat(store): persist + read per-node usage_json checkpoint column"
```

---

## Task 3: Thread usage through `WorkflowEvent` → `WorkflowSink` → wire frame

**Files:**
- Modify: `crates/bridge-workflow/src/executor.rs:80` (`WorkflowEvent::NodeFinished.usage`; emit `None` here — T4 populates it; `:523` construction)
- Modify: `crates/bridge-coordinator/src/detached.rs` — `WorkflowSink::node_finished` (`:186`), `drain_workflow` (`:215`), `DetachedProgressSink::node_finished` (`:301`), `FrameKind::NodeFinished` (`:69`), test sinks (`:524`)
- Modify: `crates/bridge-a2a-inbound/src/server.rs:1850` (`SseSink::node_finished` 4th param — ignore) + `:1200` snapshot-replay frame (`usage: None`) + `:9389` test helper
- Modify: `bin/a2a-bridge/src/main.rs:2719` (`run-workflow` printer match arm)

**Design:** `WorkflowEvent::NodeFinished`, `FrameKind::NodeFinished`, and the `WorkflowSink::node_finished` trait method all gain `usage: Option<UsageSnapshot>`. `drain_workflow` forwards it. `DetachedProgressSink` passes it to `put_node_checkpoint_sequenced` (T2) AND carries it on the live `FrameKind::NodeFinished` (`skip_serializing_if=Option::is_none` → non-panel runs byte-identical). `SseSink` ignores it (live A2A SSE plain-text surfacing DEFERRED, per SF-FIX-5).

- [ ] **Step 1: Write the failing test** — add to `detached.rs` tests (a `DetachedProgressSink` drives usage to the store + publishes a frame carrying it):

```rust
#[tokio::test]
async fn detached_sink_persists_and_publishes_node_usage() {
    let store: Arc<dyn TaskStore> = Arc::new(bridge_core::task_store::InMemoryTaskStore::new());
    let task = TaskId::parse("t-frame").unwrap();
    store.create(&working_task(&task)).await.unwrap(); // local helper inserting a Working row
    let hub = Arc::new(TaskProgressHub::new());
    let mut rx = hub.subscribe();
    let mut sink = DetachedProgressSink::new(store.clone(), task.clone(), hub.clone());

    let usage = bridge_core::orch::UsageSnapshot { used: Some(123), size: Some(1000), cost: None, at_ms: 1 };
    sink.node_finished("member", true, "OUT", Some(&usage)).await.unwrap();

    // Frame carries usage.
    let frame = rx.try_recv().unwrap();
    match frame.kind {
        FrameKind::NodeFinished { usage: Some(u), .. } => assert_eq!(u.used, Some(123)),
        other => panic!("expected NodeFinished with usage, got {other:?}"),
    }
    // Durable checkpoint carries usage.
    let cps = store.node_checkpoints(&task).await.unwrap();
    assert_eq!(cps[0].3.as_ref().unwrap().used, Some(123));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-coordinator detached_sink_persists_and_publishes_node_usage`
Expected: FAIL — `node_finished` takes 3 args; `FrameKind::NodeFinished` has no `usage`.

- [ ] **Step 3: Implement**

(a) `executor.rs:80` — add `usage: Option<bridge_core::orch::UsageSnapshot>` to `WorkflowEvent::NodeFinished`. At the construction site `:523`, emit `usage: None` for now:

```rust
                yield Ok(WorkflowEvent::NodeFinished { node: node_id.clone(), ok, output: text.clone(), usage: None });
```

(b) `detached.rs:69` — add to `FrameKind::NodeFinished`:

```rust
    NodeFinished {
        node: String,
        ok: bool,
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<bridge_core::orch::UsageSnapshot>,
    },
```

(c) `detached.rs:186` — trait method gains a 4th param:

```rust
    async fn node_finished(
        &mut self,
        _node: &str,
        _ok: bool,
        _output: &str,
        _usage: Option<&bridge_core::orch::UsageSnapshot>,
    ) -> Result<(), BridgeError> {
        Ok(())
    }
```

(d) `drain_workflow` (`:215`) — forward usage:

```rust
            Ok(WorkflowEvent::NodeFinished { node, ok, output, usage }) => {
                sink.node_finished(node.as_str(), ok, &output, usage.as_ref()).await?
            }
```

(e) `DetachedProgressSink::node_finished` (`:301`) — accept `usage: Option<&UsageSnapshot>`, pass it to `put_node_checkpoint_sequenced(..., now_ms(), usage)`, and set `usage: usage.cloned()` in the published `FrameKind::NodeFinished`.

(f) `SseSink::node_finished` (`server.rs:1850`) — add `_usage: Option<&bridge_core::orch::UsageSnapshot>` (ignored; live SSE surfacing deferred). The snapshot-replay frame builder (`server.rs:1200`, which reads from `snap.checkpoints`) sets `usage: None` (snapshot-replay surfacing deferred). The `live_node_finished` test helper (`server.rs:9389`) sets `usage: None`.

(g) `main.rs:2719` `run-workflow` printer — the match arm destructure adds `usage: _` (the CLI prints node status to stderr; usage rendering on the streaming CLI is out of scope here).

(h) Update the in-file test sink at `detached.rs:524` and any other `node_finished`/`FrameKind::NodeFinished` literal the compiler flags.

- [ ] **Step 4: Run tests**

Run: `cargo test -p bridge-coordinator && cargo test -p bridge-workflow && cargo build -p bridge-a2a-inbound -p a2a-bridge`
Expected: PASS / clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-workflow/src/executor.rs crates/bridge-coordinator/src/detached.rs crates/bridge-a2a-inbound/src/server.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(workflow): thread per-node usage through sink + wire frame"
```

---

## Task 4: Executor captures per-node usage + injects `{{workflow.costs}}`

**Files:**
- Modify: `crates/bridge-workflow/src/executor.rs` — `run_node` return type (`:102`/`:111`), the two drain loops (`:169`, `:275`), `NodeFut` (`:61`), the `outputs` map + `schedule_ready!` fan-in (`:455`, `:484-497`), `WorkflowEvent::NodeFinished` construction (`:523`)
- Add: a `render_costs_table` free fn in `executor.rs` (testable)

**Design:** `run_node` returns `(String, bool, Option<UsageSnapshot>)` — capture the LAST `Update::Usage` seen before `Done` (cumulative; last wins). `NodeFut` carries usage. The `outputs` map becomes `HashMap<String, (String, bool, Option<UsageSnapshot>)>`. At the fan-in site, for a node WITH inputs, build a `{{workflow.costs}}` markdown table from each input's captured usage. The `run_from` SEED type stays `(String,bool)` in THIS task (resumed nodes get `usage: None`); T6 extends it.

- [ ] **Step 1: Write the failing tests** — add to `executor.rs` tests:

```rust
#[tokio::test]
async fn captures_node_usage_smoke() {
    // FakeBackend that emits Usage before Done → NodeFinished.usage carries used>0.
    struct UsageBackend;
    #[async_trait::async_trait]
    impl AgentBackend for UsageBackend {
        async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(tokio_stream::iter(vec![
                Ok(Update::Text("HI".into())),
                Ok(Update::Usage(bridge_core::orch::UsageSnapshot { used: Some(15071), size: Some(258400), cost: None, at_ms: 1 })),
                Ok(Update::Done { stop_reason: "end_turn".into() }),
            ])))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { Ok(()) }
    }
    struct UReg;
    #[async_trait::async_trait]
    impl AgentRegistry for UReg {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved { entry: Arc::new(minimal_entry(id)), backend: Arc::new(UsageBackend), lease: Box::new(NoopLease) })
        }
        fn default_id(&self) -> AgentId { AgentId::parse("codex").unwrap() }
        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> { Ok(()) }
        fn list(&self) -> Vec<AgentId> { vec![] }
    }
    let ex = WorkflowExecutor::new(Arc::new(UReg));
    let evs: Vec<_> = ex.run(one_node_graph(), "DIFF".into(), "r".into(), CancellationToken::new())
        .collect::<Vec<_>>().await.into_iter().map(|r| r.unwrap()).collect();
    let nf = evs.iter().find(|e| matches!(e, WorkflowEvent::NodeFinished { .. })).unwrap();
    match nf {
        WorkflowEvent::NodeFinished { usage: Some(u), .. } => assert_eq!(u.used, Some(15071)),
        other => panic!("expected captured usage, got {other:?}"),
    }
}

#[test]
fn costs_table_renders_per_field_with_n_a() {
    use bridge_core::orch::{UsageSnapshot, UsageCost};
    let rows = vec![
        ("codexer".to_string(), Some(UsageSnapshot { used: Some(15071), size: Some(258400), cost: None, at_ms: 0 })),
        ("clauder".to_string(), Some(UsageSnapshot { used: Some(8200), size: Some(200000), cost: Some(UsageCost { amount: 0.03, currency: "USD".into() }), at_ms: 0 })),
        ("dead".to_string(), None),
    ];
    let table = render_costs_table(&rows);
    assert!(table.contains("| codexer | 15071 | 258400 |"));
    assert!(table.contains("0.03 USD"));
    assert!(table.contains("| dead | n/a | n/a | n/a | n/a |"));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p bridge-workflow captures_node_usage_smoke costs_table_renders_per_field`
Expected: FAIL — `render_costs_table` undefined; `WorkflowEvent::NodeFinished.usage` always `None`.

- [ ] **Step 3: Implement**

(a) `NodeFut` (`:61`):

```rust
type NodeFut<'a> = std::pin::Pin<
    Box<dyn futures::Future<Output = (NodeId, String, bool, Option<bridge_core::orch::UsageSnapshot>)> + Send + 'a>,
>;
```

(b) `run_node` signature (`:102`/`:111`) → `-> (String, bool, Option<bridge_core::orch::UsageSnapshot>)`. Add a `let mut last_usage: Option<UsageSnapshot> = None;` before each drain loop. In BOTH loops change the Usage arm:
- `:169` (warm): `Some(Ok(Update::Usage(u))) => { last_usage = Some(u); }`
- `:275` (cold): `Some(Ok(Update::Usage(u))) => { last_usage = Some(u); }`
Every `return (.., ..)` / final return becomes a 3-tuple: the success returns `(text, ok, last_usage)`; the early cancel/error returns use `None`.

(c) The `schedule_ready!` future (`:506-511`) — update the async block to return the 3-tuple and thread it:

```rust
    let (text, ok, usage) = this.run_node(&wf_id, &node, &vars, &run_id, &cancel, &ctx, dispatcher.as_ref()).await;
    (node.id.clone(), text, ok, usage)
```

(d) `outputs` map type (`:455`) → `HashMap<String, (String, bool, Option<UsageSnapshot>)>`; the seed adapter at `:455` wraps each `(String,bool)` seed entry as `(t, ok, None)` (resume-seeded nodes have no captured usage yet — T6 extends). The `outputs.insert` at `:525` stores the captured usage. The `outputs.get(inp)` reads at `:486`/`:495` destructure `(t, _, _)`.

(e) Fan-in injection — in `schedule_ready!` (`:484`), after building the per-input `{{<id>}}`/`{{draft}}` vars, append `{{workflow.costs}}` for nodes WITH inputs:

```rust
    if !n.inputs.is_empty() {
        let cost_rows: Vec<(String, Option<bridge_core::orch::UsageSnapshot>)> = n.inputs.iter()
            .map(|inp| (inp.as_str().to_string(), outputs.get(inp.as_str()).and_then(|(_, _, u)| u.clone())))
            .collect();
        owned.push(("workflow.costs".into(), render_costs_table(&cost_rows)));
    }
```

(f) The `WorkflowEvent::NodeFinished` construction (`:522`) — destructure the 4-tuple from `inflight.next()` and emit real usage:

```rust
    while let Some((node_id, text, ok, usage)) = inflight.next().await {
        yield Ok(WorkflowEvent::NodeFinished { node: node_id.clone(), ok, output: text.clone(), usage: usage.clone() });
        done.insert(node_id.as_str().to_string());
        outputs.insert(node_id.as_str().to_string(), (text, ok, usage));
```

(g) Add the free fn near the top of `executor.rs`:

```rust
/// Render the reserved `{{workflow.costs}}` synth var: a markdown table of each input
/// source's captured usage. Per-field `n/a` when absent (SF-FIX-1). `windowFraction = used/size`.
pub(crate) fn render_costs_table(rows: &[(String, Option<bridge_core::orch::UsageSnapshot>)]) -> String {
    let mut s = String::from("| source | used | size | window | cost |\n| --- | --- | --- | --- | --- |\n");
    for (src, usage) in rows {
        let (used, size, window, cost) = match usage {
            Some(u) => {
                let used = u.used.map(|v| v.to_string()).unwrap_or_else(|| "n/a".into());
                let size = u.size.map(|v| v.to_string()).unwrap_or_else(|| "n/a".into());
                let window = match (u.used, u.size) {
                    (Some(a), Some(b)) if b > 0 => format!("{:.1}%", (a as f64 / b as f64) * 100.0),
                    _ => "n/a".into(),
                };
                let cost = u.cost.as_ref().map(|c| format!("{} {}", c.amount, c.currency)).unwrap_or_else(|| "n/a".into());
                (used, size, window, cost)
            }
            None => ("n/a".into(), "n/a".into(), "n/a".into(), "n/a".into()),
        };
        s.push_str(&format!("| {src} | {used} | {size} | {window} | {cost} |\n"));
    }
    s
}
```

Update the existing executor tests whose `run_node`/closure destructure now sees a 3-tuple (e.g. the `failed_fan_out_leg` family) — the compiler flags them.

- [ ] **Step 4: Run tests**

Run: `cargo test -p bridge-workflow`
Expected: PASS (new + all existing).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-workflow/src/executor.rs
git commit -m "feat(workflow): capture per-node usage + inject {{workflow.costs}} at fan-in"
```

---

## Task 5: Panel weights — `WorkflowGraph.panel` + `{{workflow.weights}}` + config parse

**Files:**
- Modify: `crates/bridge-workflow/src/graph.rs` (`PanelConfig` + `WorkflowGraph.panel`)
- Modify: `crates/bridge-workflow/src/executor.rs` (`schedule_ready!` injects `{{workflow.weights}}` from `graph.panel`; add `render_weights`)
- Modify: `bin/a2a-bridge/src/config.rs` (`WorkflowToml.panel` + `PanelTomlSection` → `g.panel`)
- Modify: the **25 `WorkflowGraph { id, nodes }` struct literals** → add `panel: None`. Files: `crates/bridge-coordinator/src/detached.rs`, `crates/bridge-coordinator/src/coordinator.rs`, `crates/bridge-workflow/src/graph.rs`, `crates/bridge-workflow/src/executor.rs`, `crates/bridge-mcp/tests/mcp_client.rs`, `crates/bridge-a2a-inbound/tests/workflow_producer.rs`, `bin/a2a-bridge/tests/integration_run_workflow.rs`, `bin/a2a-bridge/src/config.rs` (the `load_workflows` builder), `bin/a2a-bridge/src/main.rs`.

**Design:** Weights live on `WorkflowGraph` (intrinsic to the workflow def → serialize into the durable spec snapshot → resume for free). `PanelConfig { weights: BTreeMap<String, f64> }` (BTreeMap = deterministic render order). `#[serde(default, skip_serializing_if = "Option::is_none")]` keeps old snapshots deserializable + absent-panel graphs byte-identical on the wire (W3b additive-safe). The executor injects `{{workflow.weights}}` for ALL nodes (harmless; only the synth prompt references it).

- [ ] **Step 1: Write the failing tests**

In `graph.rs` tests:

```rust
#[test]
fn graph_panel_serde_is_additive() {
    let mut weights = std::collections::BTreeMap::new();
    weights.insert("usage".to_string(), 0.2);
    weights.insert("benefit".to_string(), 0.4);
    let g = WorkflowGraph {
        id: WorkflowId::parse("panel").unwrap(),
        nodes: vec![WorkflowNode { id: NodeId::parse("a").unwrap(), agent: AgentId::parse("x").unwrap(), prompt_template: "{{input}}".into(), inputs: vec![] }],
        panel: Some(PanelConfig { weights }),
    };
    let s = serde_json::to_string(&g).unwrap();
    assert!(s.contains("\"benefit\":0.4"));
    let back: WorkflowGraph = serde_json::from_str(&s).unwrap();
    assert_eq!(back.panel.unwrap().weights["usage"], 0.2);
    // Old snapshot with no `panel` key → None.
    let old: WorkflowGraph = serde_json::from_str(r#"{"id":"w","nodes":[{"id":"a","agent":"x","prompt_template":"{{input}}","inputs":[]}]}"#).unwrap();
    assert!(old.panel.is_none());
}
```

In `executor.rs` tests:

```rust
#[test]
fn weights_render_sorted() {
    let mut w = std::collections::BTreeMap::new();
    w.insert("risk".to_string(), 0.3);
    w.insert("benefit".to_string(), 0.4);
    let out = render_weights(&Some(crate::graph::PanelConfig { weights: w }));
    assert_eq!(out, "- benefit: 0.4\n- risk: 0.3\n"); // BTreeMap → sorted by key
    assert_eq!(render_weights(&None), "(no weights configured)");
}
```

In `config.rs` tests (mirroring `parses_workflows_and_loads_prompts`):

```rust
#[test]
fn parses_workflow_panel_weights() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("p.md"), "go {{input}} {{workflow.weights}}").unwrap();
    let toml = format!(
        "{AGENTS_HEADER}\n[[workflows]]\nid = \"panel\"\n[workflows.panel]\nweights = {{ usage = 0.2, benefit = 0.4 }}\n\
         [[workflows.nodes]]\nid = \"only\"\nagent = \"codex\"\nprompt_file = \"p.md\"\ninputs = []\n{SERVER_FOOTER}"
    );
    let cfg: BridgeConfig = toml::from_str(&toml).unwrap();
    let map = cfg.load_workflows(dir.path()).unwrap();
    let g = map.get(&WorkflowId::parse("panel").unwrap()).unwrap();
    assert_eq!(g.panel.as_ref().unwrap().weights["benefit"], 0.4);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p bridge-workflow graph_panel_serde_is_additive weights_render_sorted && cargo test -p a2a-bridge parses_workflow_panel_weights`
Expected: FAIL — `PanelConfig` / `panel` field / `render_weights` undefined.

- [ ] **Step 3: Implement**

(a) `graph.rs` — add:

```rust
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PanelConfig {
    #[serde(default)]
    pub weights: std::collections::BTreeMap<String, f64>,
}
```

and to `WorkflowGraph`:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<PanelConfig>,
```

(b) `executor.rs` — add the free fn:

```rust
pub(crate) fn render_weights(panel: &Option<crate::graph::PanelConfig>) -> String {
    match panel {
        Some(p) if !p.weights.is_empty() => {
            let mut s = String::new();
            for (k, v) in &p.weights { s.push_str(&format!("- {k}: {v}\n")); }
            s
        }
        _ => "(no weights configured)".to_string(),
    }
}
```

In `schedule_ready!` (`:484`), after the `{{workflow.costs}}` push, inject weights for every node:

```rust
    owned.push(("workflow.weights".into(), render_weights(&graph.panel)));
```

(`graph` is in scope in the macro body.)

(c) `config.rs` — extend `WorkflowToml` (`:156`) with `#[serde(default)] pub panel: Option<PanelTomlSection>` and add:

```rust
#[derive(Debug, serde::Deserialize)]
pub struct PanelTomlSection {
    #[serde(default)]
    pub weights: std::collections::BTreeMap<String, f64>,
}
```

In `load_workflows` (`:890`), set the field when building the graph:

```rust
            let g = WorkflowGraph {
                id: id.clone(),
                nodes,
                panel: w.panel.as_ref().map(|p| bridge_workflow::graph::PanelConfig { weights: p.weights.clone() }),
            };
```

(d) Add `panel: None` to the remaining 24 `WorkflowGraph { id, nodes }` literals across the file list above. Mechanical; the compiler enumerates every site.

- [ ] **Step 4: Run tests**

Run: `cargo test -p bridge-workflow && cargo test -p a2a-bridge parses_workflow_panel_weights && cargo build --workspace --all-targets`
Expected: PASS / clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-workflow/src/graph.rs crates/bridge-workflow/src/executor.rs bin/a2a-bridge/src/config.rs crates/bridge-coordinator/src crates/bridge-mcp/tests/mcp_client.rs crates/bridge-a2a-inbound/tests/workflow_producer.rs bin/a2a-bridge/tests/integration_run_workflow.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(workflow): [workflows.panel] weights + {{workflow.weights}} synth var"
```

---

## Task 6: Resume carries captured usage (seed type + resume routine)

**Files:**
- Modify: `crates/bridge-workflow/src/executor.rs` — `run_from`/`run_from_with_context`/`run_from_with_context_inner` seed type `HashMap<String, (String, bool)>` → `HashMap<String, (String, bool, Option<UsageSnapshot>)>`; the `outputs` seed adapter (`:455`) consumes the 3-tuple directly
- Modify: `crates/bridge-coordinator/src/detached.rs` — `spawn_detached_workflow` seed param (`:1115`); `resume_working_tasks` seed build (`:1423`) reads usage from `node_checkpoints` (now a 4-tuple from T2); the in-test seeds at `:1021`/`:1173`

**Design:** The resume seed carries each already-finished node's usage from its `usage_json` checkpoint, so a resumed synth's `{{workflow.costs}}` shows the members' REAL usage WITHOUT re-running them (SF-FIX-4 crash-resume proof). A pre-B2 / crashed-before-usage node seeds `None` → `n/a`.

- [ ] **Step 1: Write the failing test** — add to `executor.rs` tests (resume seed with usage → synth's `{{workflow.costs}}` shows it):

```rust
#[tokio::test]
async fn resumed_synth_sees_seeded_member_usage() {
    use bridge_core::orch::UsageSnapshot;
    let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
    let reg = Arc::new(FakeRegistry { backends: [("synth".to_string(), mk("FINAL"))].into() });
    let synth_rec = reg.backends.get("synth").unwrap().1.clone();
    let ex = WorkflowExecutor::new(reg);
    // Seed the two members as done WITH usage; only synth runs.
    let mut seed: HashMap<String, (String, bool, Option<UsageSnapshot>)> = HashMap::new();
    seed.insert("codex".into(), ("CODEX_REVIEW".into(), true, Some(UsageSnapshot { used: Some(15071), size: Some(258400), cost: None, at_ms: 0 })));
    seed.insert("claude".into(), ("CLAUDE_REVIEW".into(), true, None));
    let _ = ex.run_from(review_graph(), "DIFF".into(), "r".into(), CancellationToken::new(), seed)
        .collect::<Vec<_>>().await;
    let p = &synth_rec.prompts.lock().unwrap()[0];
    assert!(p.contains("| codex | 15071 | 258400 |"), "resumed synth costs table shows seeded member usage: {p}");
    assert!(p.contains("| claude | n/a |"), "member with no captured usage → n/a");
}
```

(`review_graph`'s synth prompt must reference `{{workflow.costs}}` — extend the in-test `review_graph` synth template to `"merge {{codex}} + {{claude}} for {{input}}\n{{workflow.costs}}"`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-workflow resumed_synth_sees_seeded_member_usage`
Expected: FAIL — `run_from` seed type is the 2-tuple.

- [ ] **Step 3: Implement**

(a) `executor.rs` — change the seed type on `run_from` (`:373`), `run_from_with_context` (`:394`), `run_from_with_context_and_dispatcher` (`:407`), `run_from_with_context_inner` (`:421`) to `HashMap<String, (String, bool, Option<bridge_core::orch::UsageSnapshot>)>`. The `outputs` map is now the SAME type (T4), so the `let mut outputs = seed;` adapter at `:455` no longer needs wrapping — remove the T4 `(t, ok, None)` wrap and assign the seed directly. The closure-under-`inputs` validation (`:442`) is unchanged.

(b) `detached.rs:1115` — `spawn_detached_workflow` seed param → the 3-tuple type.

(c) `resume_working_tasks` (`:1423`) — build the seed from the 4-tuple `node_checkpoints`:

```rust
        let seed: std::collections::HashMap<String, (String, bool, Option<bridge_core::orch::UsageSnapshot>)> = cps
            .iter()
            .map(|(node, output, ok, usage)| (node.as_str().to_string(), (output.clone(), *ok, usage.clone())))
            .collect();
```

The terminal short-circuit `seed.get(&terminal_id)` (`:1461`) destructure becomes `(output, ok, _usage)`.

(d) The in-test seeds at `detached.rs:1021`/`:1173` and any other `run_from*` caller — update to the 3-tuple (the compiler flags them).

- [ ] **Step 4: Run tests**

Run: `cargo test -p bridge-workflow && cargo test -p bridge-coordinator`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-workflow/src/executor.rs crates/bridge-coordinator/src/detached.rs
git commit -m "feat(workflow): resume seed carries per-node usage (crash-resume proof)"
```

---

## Task 7: The `panel` workflow — config + prompts

**Files:**
- Create: `prompts/panel-member.md` (one panel member's analysis prompt)
- Create: `prompts/panel-synth.md` (the weighted-synth contract — references `{{workflow.costs}}` + `{{workflow.weights}}`)
- Create: `examples/a2a-bridge.panel.toml` (fan-out 2 members → weighted synth, with `[workflows.panel] weights`)
- Test: `bin/a2a-bridge/tests/` (a config-load + render integration test, OR extend `config.rs` unit tests)

**Design:** Two members (codex + claude, or codex@low + codex@high) fan out over the same input; the synth applies the configured weights and the captured usage. The prompts follow the existing bounded-STOP / no-tools review-synth contract (`prompts/review-synth.md`).

- [ ] **Step 1: Write the failing test** — add to `config.rs` tests (the shipped `panel` config loads + validates + the synth prompt wires both reserved vars):

```rust
#[test]
fn shipped_panel_config_loads_and_wires_reserved_vars() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let raw = std::fs::read_to_string(base.join("a2a-bridge.panel.toml")).unwrap();
    let cfg: BridgeConfig = toml::from_str(&raw).unwrap();
    let map = cfg.load_workflows(&base).unwrap();
    let g = map.get(&WorkflowId::parse("panel").unwrap()).expect("panel workflow present");
    assert!(g.panel.as_ref().unwrap().weights.contains_key("usage"));
    let synth = g.nodes.iter().find(|n| n.id.as_str() == "synth").unwrap();
    assert!(synth.prompt_template.contains("{{workflow.costs}}"));
    assert!(synth.prompt_template.contains("{{workflow.weights}}"));
    g.validate().unwrap();
}
```

(Adjust the `base` path to however the existing config tests locate `examples/`; if they read prompts relative to the config dir, mirror that. The prompt `prompt_file` paths in the TOML are relative to `base` per `load_workflows`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge shipped_panel_config_loads_and_wires_reserved_vars`
Expected: FAIL — `examples/a2a-bridge.panel.toml` does not exist.

- [ ] **Step 3: Create the files**

`prompts/panel-member.md`:

```markdown
You are ONE member of an analysis panel. Independently analyze the input below.

OUTPUT CONTRACT — follow exactly:
- Respond with your analysis as plain text ONLY, directly in this reply.
- Do NOT use any tools. Do NOT read or write files. Do NOT run commands or searches. Everything you need is below.
- When your analysis is complete, STOP.

Cover, in clearly labeled sections: **Pros**, **Cons**, **Benefit**, **Risk**. Be specific and concise.

=== INPUT ===
{{input}}
```

`prompts/panel-synth.md`:

```markdown
Synthesize ONE weighted panel recommendation from the independent member analyses below.

OUTPUT CONTRACT — follow exactly:
- Respond with the panel as plain text ONLY, directly in this reply.
- Do NOT use any tools. Do NOT read or write files. Do NOT run commands or searches. Everything you need is below.
- When the panel is complete, STOP.

HOW TO BUILD THE PANEL:
1. For EACH member, a compact block: **Pros / Cons / Usage / Benefit / Risk**. Take Usage from the table below — reproduce its row for that member VERBATIM (do not invent numbers).
2. Apply the operator-configured WEIGHTS below to reach a weighted recommendation. State the weights you applied and name the winner + why in one line.
3. If a member reported an error marker instead of an analysis (a node failed), note the lens is missing, synthesize from the survivors, and show its usage as n/a.

=== OPERATOR WEIGHTS ===
{{workflow.weights}}

=== PER-MEMBER USAGE (real, captured) ===
{{workflow.costs}}

=== MEMBER A ===
{{member_a}}

=== MEMBER B ===
{{member_b}}

(Original input, for reference: {{input}})
```

`examples/a2a-bridge.panel.toml` (model after `examples/a2a-bridge.workflows.toml`):

```toml
default = "codex"

[[agents]]
id = "codex"
cmd = "codex-acp"

[[agents]]
id = "claude"
cmd = "claude-agent-acp"

[[workflows]]
id = "panel"

[workflows.panel]
weights = { usage = 0.2, benefit = 0.4, risk = 0.3, pros = 0.05, cons = 0.05 }

[[workflows.nodes]]
id = "member_a"
agent = "codex"
prompt_file = "../prompts/panel-member.md"
inputs = []

[[workflows.nodes]]
id = "member_b"
agent = "claude"
prompt_file = "../prompts/panel-member.md"
inputs = []

[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "../prompts/panel-synth.md"
inputs = ["member_a", "member_b"]

[server]
addr = "127.0.0.1:8080"
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p a2a-bridge shipped_panel_config_loads_and_wires_reserved_vars`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add prompts/panel-member.md prompts/panel-synth.md examples/a2a-bridge.panel.toml bin/a2a-bridge/src/config.rs
git commit -m "feat(workflow): ship the panel workflow (config + prompts)"
```

---

## Task 8: Degrade case + workspace gate

**Files:**
- Test: `crates/bridge-workflow/src/executor.rs` (degrade: one member fails → survivor synthesizes, failed member usage `n/a`)

**Design:** Verify the SF-FIX live-gate's degrade case at the unit level: a panel where one member's node fails (resolve error → error marker, `usage: None`) still synthesizes from the survivor, and the failed member's row in `{{workflow.costs}}` is `n/a`.

- [ ] **Step 1: Write the failing/regression test** — add to `executor.rs` tests:

```rust
#[tokio::test]
async fn panel_degrades_failed_member_usage_is_n_a() {
    // No "member_a" backend registered → its node fails (error marker, usage None);
    // member_b + synth still run, synth's costs table shows member_a as n/a.
    let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
    let reg = Arc::new(FakeRegistry { backends: [
        ("member_b".to_string(), mk("B_ANALYSIS")),
        ("synth".to_string(), mk("PANEL")),
    ].into() });
    let synth_rec = reg.backends.get("synth").unwrap().1.clone();
    let g = Arc::new(WorkflowGraph {
        id: WorkflowId::parse("panel").unwrap(),
        nodes: vec![
            WorkflowNode { id: NodeId::parse("member_a").unwrap(), agent: AgentId::parse("member_a").unwrap(), prompt_template: "{{input}}".into(), inputs: vec![] },
            WorkflowNode { id: NodeId::parse("member_b").unwrap(), agent: AgentId::parse("member_b").unwrap(), prompt_template: "{{input}}".into(), inputs: vec![] },
            WorkflowNode { id: NodeId::parse("synth").unwrap(), agent: AgentId::parse("synth").unwrap(), prompt_template: "{{member_b}}\n{{workflow.costs}}".into(), inputs: vec![NodeId::parse("member_a").unwrap(), NodeId::parse("member_b").unwrap()] },
        ],
        panel: None,
    });
    let evs: Vec<_> = WorkflowExecutor::new(reg).run(g, "DIFF".into(), "r".into(), CancellationToken::new())
        .collect::<Vec<_>>().await;
    assert!(matches!(evs.last().unwrap().as_ref().unwrap(), WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, .. }));
    let p = &synth_rec.prompts.lock().unwrap()[0];
    assert!(p.contains("| member_a | n/a | n/a | n/a | n/a |"), "failed member → n/a row: {p}");
}
```

- [ ] **Step 2: Run to verify it fails (or passes if T4 already covers)**

Run: `cargo test -p bridge-workflow panel_degrades_failed_member_usage_is_n_a`
Expected: PASS (the behavior is already implemented by T4's `render_costs_table` + degrade path; this test locks it).

- [ ] **Step 3: Full workspace gate** (controller, in the clean host env — codex's sandbox can't run runtime tests; this is where the Slice-9 stale-count lesson applies):

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --all-targets`
Expected: clean fmt, no clippy warnings, all tests pass (watch for any stale cross-crate test count — the `cargo test --workspace` run is what catches them).

- [ ] **Step 4: Commit**

```bash
git add crates/bridge-workflow/src/executor.rs
git commit -m "test(workflow): panel degrade — failed member usage is n/a"
```

---

## After the tasks (process — not plan steps)

1. **Whole-branch dual-lens review** (codex xhigh read-only + Opus architecture lens) over the full diff vs `main`. Fold blockers/majors. (The Slice-9 lesson: this catches what per-task tests + the happy-path gate miss.)
2. **Live-gate** (per spec v2 "Updated live-gate") with the `panel` workflow + two intentionally-different members + a `[workflows.panel] weights` table:
   - (a) each member node's RAW `usage.used > 0`, distinct per member (read the checkpoint `usage_json` / `NodeFinished.usage` frame — NOT the synth markdown);
   - (b) the synth artifact reproduces the generated `{{workflow.costs}}` table verbatim + states the weights + a weighted recommendation;
   - (c) crash-resume mid-run (kill `serve` after members finish, restart) → resumed synth's `{{workflow.costs}}` still shows the members' usage (proves SF-FIX-4);
   - (d) degrade: one member fails → survivor synthesizes + its usage shows `n/a`.
   Reuse a codex-HIGH impl config + the spec-review scaffolding port pattern.
3. **Merge** `--no-ff` to `main` + push; update memory (`slice-10-shipped`) + the orchestration handoff.

**Staging discipline (carry-in):** stage ONLY each task's files. The worktree has many pre-existing untracked `examples/*.toml` / `prompts/*.md` + a pre-existing `M examples/a2a-bridge.slicing-analysis.toml` — NEVER fold them. (T7 adds NEW `examples/a2a-bridge.panel.toml` + `prompts/panel-*.md` — stage those explicitly by name.)

**Tracked deferrals (note in the PR + memory):** snapshot-replay `task watch` usage surfacing (`server.rs:1200` + `TaskProgressSnapshot.checkpoints`); live A2A SSE per-node usage (`SseSink` plain text, `server.rs:1841`); machine-readable JSON panel (ADR-0012 structuring node); native `fan_out` op.

---

## Self-Review (run against the spec)

**1. Spec coverage (SF-FIX-1..6):**
- SF-FIX-1 (usage not cost; per-field `n/a`; money only when present): T4 `render_costs_table` (per-field n/a, `windowFraction`, cost only when `Some`). ✅
- SF-FIX-2 (operator-config weights → `{{workflow.weights}}`): T5 (`[workflows.panel] weights` → `PanelConfig` → injection). ✅
- SF-FIX-3 (reserved `workflow.` namespace, collision-proof): T4/T5 use `workflow.costs`/`workflow.weights`; verified `NodeId` charset bans `.`. ✅
- SF-FIX-4 (usage survives crash-resume): T1 (journal event) + T2 (checkpoint column + `node_checkpoints` return) + T6 (seed/`run_from`/`resume_working_tasks` carry usage). ✅ Old data folds as `None` (T1/T2 tests).
- SF-FIX-5 (surface on detached `task watch` `FrameKind::NodeFinished`, gate on RAW fields; live SSE deferred): T3 (`FrameKind::NodeFinished.usage`, `skip_serializing_if`; `SseSink` ignores). The live-gate asserts raw checkpoint/frame usage. ✅ Snapshot-replay surfacing explicitly deferred.
- SF-FIX-6 (substrate = workflow DAG executor, not `fanout.rs`): all executor/coordinator work; `fanout.rs` untouched. ✅

**2. Placeholder scan:** every code step shows real code; no "TBD"/"add error handling"/"similar to Task N". ✅

**3. Type consistency:** `usage: Option<UsageSnapshot>` is the single carrier across `OrchEventKind::NodeFinished` (T1), `put_node_checkpoint_sequenced`/`node_checkpoints` (T2), `WorkflowEvent::NodeFinished` + `FrameKind::NodeFinished` + `WorkflowSink::node_finished` (T3), `run_node` return + `outputs` map (T4), `run_from` seed (T6). `render_costs_table(&[(String, Option<UsageSnapshot>)])` and `render_weights(&Option<PanelConfig>)` signatures match their call sites. `PanelConfig { weights: BTreeMap<String, f64> }` consistent in graph + config. ✅

**4. Open decision flagged for plan-review:** weights on `WorkflowGraph.panel` (durable-for-free via the spec snapshot, ~24 trivial literal edits) vs. `WorkflowRunContext` (fewer edits, but needs envelope + resume re-derivation). This plan chose the graph field — confirm in the dual plan-review.

---

## v2 — dual plan-review folded (codex xhigh needs-revision + Opus needs-revision) — BINDING

> Supersedes the task bodies above where it conflicts. Both lenses returned **needs-revision** with **no
> re-architecture** — both CONFIRMED the core design: panel-on-`WorkflowGraph` is additive-safe and is serialized
> through `encode_workflow_spec` (`detached.rs:1323`); `WorkflowSpecEnvelope` deserializes it (old snapshots →
> `None`); `NodeId` bans `.` (`ids.rs:58`); the renderer accepts dotted tokens (`template.rs:16`); the resume seed
> reads `node_checkpoints` directly (`detached.rs:1415`). The fixes below are tactical (compile-green ripples +
> test strength + one contract detail). Apply each in its named task.

### PR-FIX-1 (codex BLOCKER #1 + Opus M1) — T2 must update EVERY `TaskStore` impl AND direct call site
The `node_checkpoints` 3-tuple→4-tuple + `put_node_checkpoint_sequenced` +`usage` arity changes ripple to **all
five** `impl TaskStore` sites — not the three T2 lists. Add to T2:
- `FailingCheckpointStore` @ `crates/bridge-coordinator/src/detached.rs:722` (and its wrapper methods ~`:766`)
- `LegacyFallbackStore` @ `crates/bridge-a2a-inbound/src/server.rs:8760` (wrapper methods ~`:8820`)
- plus **direct positional call sites** of `put_node_checkpoint_sequenced` in `server.rs` tests (e.g. `:8916`) —
  add the trailing `None` argument.
T2's gate becomes `cargo test --workspace --all-targets` (NOT per-crate) so a wrapper/test impl in another crate
can't silently break. (The single `WorkflowSink` trait def at `detached.rs:182` is confirmed — that part of T3 is fine.)

### PR-FIX-2 (codex BLOCKER #2) — T4 must fix the terminal destructure
After `outputs` becomes `(String, bool, Option<UsageSnapshot>)`, `executor.rs:538`
(`let (term_text, term_ok) = outputs.get(&terminal_id).cloned().unwrap_or_default();`) no longer compiles. Change
to `let (term_text, term_ok, _usage) = outputs.get(&terminal_id).cloned().unwrap_or_default();`
(the 3-tuple is still `Default`, so `unwrap_or_default()` stays valid). Add this as an explicit T4 step.

### PR-FIX-3 (codex MAJOR #3 + the Slice-9 lesson) — every task gates with `--all-targets`
`WorkflowEvent::NodeFinished` test literals live in `bin/a2a-bridge/src/review.rs:865/870/895` (a file the draft
missed). `cargo build -p a2a-bridge` does NOT compile test targets, so a `--bin`/`build` gate false-greens. **Every
task** that changes a public type/signature ends with `cargo test --workspace --all-targets` (or at minimum
`cargo test -p <crate> --all-targets`). T3's file list adds `bin/a2a-bridge/src/review.rs`. (This is the Slice-9
MCP 6→8 stale-count lesson, generalized.)

### PR-FIX-4 (codex MAJOR #4) — the usage column is `windowFraction = used/size` (raw fraction), not a percent
SF-FIX-1's contract is `{used, size, windowFraction}` where `windowFraction = used/size` (a raw fraction, e.g.
`0.0583` — see `session_manager.rs:140`). T4's `render_costs_table` must emit a **`windowFraction`** column with
the RAW fraction, not a `window` percent string. Revised helper body for the fraction cell:

```rust
let window = match (u.used, u.size) {
    (Some(a), Some(b)) if b > 0 => format!("{:.4}", a as f64 / b as f64),
    _ => "n/a".into(),
};
```

and the header → `"| source | used | size | windowFraction | cost |\n| --- | --- | --- | --- | --- |\n"`. Update
the T4 `costs_table_renders_per_field_with_n_a` test to assert the derived fraction (e.g.
`assert!(table.contains("| codexer | 15071 | 258400 | 0.0583 |"));`) AND the `n/a` row — i.e. assert the COMPUTED
field, not just `used`/`size`.

### PR-FIX-5 (codex MAJOR #5) — T6 must prove SF-FIX-4 at the COORDINATOR resume level, not just `run_from`
The draft's T6 test only feeds a pre-built seed to `run_from` — it doesn't exercise the store→resume-scan→spawn
chain. Keep that unit test, but ADD a `bridge-coordinator` resume test (in `detached.rs` tests): create a Working
task with a `panel`-shaped graph snapshot + member `usage_json` checkpoints (via `put_node_checkpoint_sequenced`
with `Some(usage)`), call `resume_working_tasks(&deps, cap)`, and assert the resumed **synth's prompt** received
the `{{workflow.costs}}` table with the members' usage. (Use the existing detached-test harness — the
`spawn_detached_workflow_*_for_test` helpers + a fake dispatcher/registry that records the synth prompt.) This is
the real SF-FIX-4 crash-resume proof; it belongs in T6, not deferred to the live-gate.

### PR-FIX-6 (codex MAJOR #6) — T2 must test the DURABLE JOURNAL leg + the legacy-schema migration
T2's draft test only checks `node_checkpoints`. Add, in T2:
- assert `journal_from(&task, -1)` returns an `OrchEventKind::NodeFinished { usage: Some(..), .. }` for the
  usage-bearing checkpoint — for BOTH `SqliteStore` and `MemoryTaskStore`.
- extend the existing legacy-schema migration test (`sqlite.rs:1280`, pre-creates the old `task_node_checkpoints`)
  to insert a row with NO `usage_json`, then assert `node_checkpoints` reads `usage = None` (proves the
  `ALTER TABLE ADD COLUMN usage_json` migration + the NULL→None read on legacy rows).

### PR-FIX-7 (codex MINOR #7) — inject `{{workflow.weights}}` only for nodes WITH inputs (match `{{workflow.costs}}`)
In T5, move the `owned.push(("workflow.weights"...))` inside the same `if !n.inputs.is_empty() { … }` block as
`{{workflow.costs}}` — the reserved synth vars surface only at fan-in (synth) nodes, keeping root members'
prompts byte-identical. Update the T5 `weights_render_sorted` test target accordingly (it tests `render_weights`
directly, so it's unaffected; just ensure an injection test, if added, uses a fan-in node).

### PR-FIX-8 (Opus m1) — test-code name fixes
The in-memory store type is **`MemoryTaskStore`** (`task_store.rs:366`), NOT `InMemoryTaskStore` — fix every test
that constructs it (T3's `detached_sink_persists_and_publishes_node_usage`, any other). Replace the invented
`sample_task`/`working_task`/`working_task` helpers with an inline `TaskRecord { … status: Working … }` + `create`
(or bind to a confirmed existing helper in that test module) so the tests compile.

### PR-FIX-9 (codex NIT #8) — the literal count is ~24, compiler-enumerated
Drop the exact "25" — `rg "WorkflowGraph {"` over-counts (a fn signature at `main.rs:4556`, the struct def at
`graph.rs:6`). T5 relies on the COMPILER to enumerate the real construction sites; the file list stays, the number
is approximate.

### PR-FIX-10 (Opus C1 — clarification, no code) — `task watch` needs NO printer change
`task_watch_cmd` (`main.rs:3236`) prints the raw `data:` SSE payload verbatim, so T3's `FrameKind::NodeFinished.usage`
(`skip_serializing_if=Option::is_none`) surfaces automatically → **SF-FIX-5 is satisfied by the frame field alone**;
do NOT add a `task watch` printer task. The live-gate reads usage from `task watch` raw output. (Distinct from
PR-FIX-3's `review.rs` literals, which ARE a real edit.)

### Verdict reconciliation
Both lenses: **needs-revision, no re-architecture.** Confirmed sound (do not reopen): panel-on-graph (durable +
resume-safe via the snapshot, with the verified advantage that `WorkflowRunContext` would LOSE weights on resume);
the direct-`node_checkpoints` resume seed (the fold/`TaskProgressSnapshot.checkpoints` stays a usage-less 4-tuple);
the reserved `workflow.` var namespace. After folding PR-FIX-1..10 the plan is **ready-to-implement**.
