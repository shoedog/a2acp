# W3b — Node checkpoint history + crash-resume Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A detached workflow that is `Working` when `serve` restarts resumes from per-node checkpoints — re-running only not-yet-finished nodes, reusing finished nodes' outputs — instead of being swept to `Interrupted`.

**Architecture:** Completion-driven executor scheduling (so a fan-out leg is checkpointable the instant it finishes) + an additive `NodeFinished{output}` over a now-fallible `WorkflowSink` + a `run_from(seed)` resume entry (executor stays pure) + a lean `task_node_checkpoints` table & a versioned graph snapshot in `tasks` + an auto-resume boot routine on `InboundServer` with an attempt cap.

**Tech Stack:** Rust, tokio, `futures::stream::FuturesUnordered`, rusqlite, serde, the existing `TaskStore`/`WorkflowExecutor`/detached-runner from W3a.

**Spec:** `docs/superpowers/specs/2026-06-03-a2a-bridge-w3b-resume-design.md` (rev1, dual-designed + dual-reviewed).

**Branch:** `feat/w3b-resume` off `main`.

**Grounding facts (confirmed against the code):**
- Executor `run()` (executor.rs ~129-176) builds `outputs: HashMap<String,(String,bool)>` + `done: HashSet<String>`, loops `while done.len() < graph.nodes.len()`, filters `ready` (not done + all inputs done), yields `NodeStarted` per ready node, runs them via `futures::future::join_all`, then per result yields `NodeFinished{node,ok}` + `done.insert` + `outputs.insert`. Terminal = `outputs[terminal_id]`. `run_node(&self, wf_id, node, vars:&HashMap<&str,&str>, run_id, cancel) -> (String,bool)`.
- `WorkflowEvent::NodeFinished{node: NodeId, ok: bool}` (NO output today). `WorkflowSink` trait methods return `()`; `drain_workflow(stream, sink) -> bool` (terminal_seen). `SseSink`, `TaskStoreSink`(+`take()`), `Finalizer`, `now_ms()` in workflow_sink.rs.
- `WorkflowGraph`/`WorkflowNode` derive `Debug, Clone` ONLY (graph.rs); `NodeId`/`WorkflowId`/`AgentId` derive serde (ids.rs). `bridge-workflow/Cargo.toml` has no `serde` dep.
- `TaskStore` (task_store.rs) has create/set_terminal/get/list/sweep_interrupted/cancel_if_working; `TaskRecord{id,workflow,status,result,error,created_ms,updated_ms}`. `SqliteStore` `tasks` table + `create_schema` (execute_batch); `open(path)` holds an `fs2` lock; `MemoryTaskStore` is the in-mem default.
- The detached arm (server.rs `unary_message` `RouteTarget::Workflow(ref wf_id)`) creates `TaskRecord{Working}` + registers a token + `spawn_detached_workflow(&srv, task, text_parts, wf_id.clone(), token)` + returns `a2a::Task{Working}`. `spawn_detached_workflow(srv,task,text_parts,wf_id,token)->JoinHandle`. `InboundServer` fields incl. `executor: Option<Arc<WorkflowExecutor>>`, `workflows: Arc<HashMap<WorkflowId,Arc<WorkflowGraph>>>`, `task_store: Arc<dyn TaskStore>`, `workflow_cancels: Arc<Mutex<HashMap<TaskId,CancellationToken>>>`.
- serve (main.rs) opens the store (file→`SqliteStore::open`+sweep, else Memory), `.with_task_store`, binds. `StoreConfig{path}`. Tests: workflow_producer.rs under `-p bridge-a2a-inbound`; `-p a2a-bridge` has no `--lib`. The W1 streaming guard test: `streaming_workflow_emits_node_status_synth_artifact_and_completed` (workflow_producer.rs:354).

---

## File Structure
- **Modify** `crates/bridge-workflow/src/executor.rs` — completion-driven scheduling; `NodeFinished{output}`; `run_from(seed)`.
- **Modify** `crates/bridge-workflow/src/graph.rs` + `Cargo.toml` — serde derives + dep.
- **Modify** `crates/bridge-a2a-inbound/src/workflow_sink.rs` — fallible sink + `drain_workflow` abort; `TaskStoreSink` checkpoints.
- **Modify** `crates/bridge-a2a-inbound/src/server.rs` — `SseSink`/producer for new sigs; `spawn_detached_workflow` injected-graph; the boot resume routine on `InboundServer`; persist input+snapshot in the detached arm.
- **Modify** `crates/bridge-core/src/task_store.rs` — `TaskStore` methods + `ResumeClaim` + `TaskRecord` fields + `MemoryTaskStore`.
- **Modify** `crates/bridge-store/src/sqlite.rs` — migration + `task_node_checkpoints` + impls + `PRAGMA foreign_keys=ON`.
- **Modify** `bin/a2a-bridge/src/{config.rs,main.rs}` — `resume_attempt_cap`; serve calls the resume routine.
- **Modify** tests in `crates/bridge-workflow/src/executor.rs` (tests mod) + `crates/bridge-a2a-inbound/tests/workflow_producer.rs`.
- **Create** `docs/adr/0011-crash-resume.md`.

---

## Phase A — Completion-driven scheduling (ISOLATED, behavior-preserving FIRST)

### Task 1: replace `join_all` with `FuturesUnordered` completion-driven scheduling

**Files:** Modify `crates/bridge-workflow/src/executor.rs` (the `run()` stream body).

**Goal:** schedule the whole DAG with one in-flight set: spawn each ready node's future into a `FuturesUnordered`; as each completes, yield its `NodeFinished`, update `done`+`outputs`, and add any newly-ready nodes — instead of batch-`join_all`. Preserve: the write-ahead barrier (the `yield NodeFinished` happens before scheduling dependents, and `drain_workflow` awaits the sink between yields — see Phase B), the cancel behavior, the no-terminal/degradation semantics, and the final result.

- [ ] **Step 1: Confirm the regression guard fails-safe.** Run the existing streaming test to confirm it's green BEFORE the change:
  Run: `cargo test -p bridge-a2a-inbound --test workflow_producer streaming_workflow_emits_node_status_synth_artifact_and_completed`
  Expected: PASS (this is the behavior the rewrite must preserve).

- [ ] **Step 2: Rewrite the `run()` loop body.** Replace the `while done.len() < graph.nodes.len() { … join_all … }` block (executor.rs ~143-169) with a completion-driven version. Keep the surrounding `Box::pin(async_stream::stream! { … })`, the `outputs`/`done` declarations, `terminal_id`, and the terminal computation after the loop. New loop:

```rust
            use futures::stream::FuturesUnordered;
            let mut inflight: FuturesUnordered<_> = FuturesUnordered::new();
            let mut scheduled: std::collections::HashSet<String> = std::collections::HashSet::new();

            // Schedule every node whose inputs are all done and which isn't done/scheduled.
            // Returns the node ids newly scheduled (so the caller can emit NodeStarted).
            macro_rules! schedule_ready {
                () => {{
                    let mut started: Vec<NodeId> = Vec::new();
                    for n in graph.nodes.iter() {
                        let id = n.id.as_str();
                        if done.contains(id) || scheduled.contains(id) {
                            continue;
                        }
                        if n.inputs.iter().all(|i| done.contains(i.as_str())) {
                            scheduled.insert(id.to_string());
                            started.push(n.id.clone());
                            let mut owned: Vec<(String, String)> = vec![("input".into(), input.clone())];
                            for inp in &n.inputs {
                                if let Some((t, _)) = outputs.get(inp.as_str()) {
                                    owned.push((inp.as_str().into(), t.clone()));
                                }
                            }
                            let node = n.clone();
                            let run_id = run_id.clone();
                            let cancel = cancel.clone();
                            let wf_id = graph.id.as_str().to_string();
                            let this = &this;
                            inflight.push(async move {
                                let vars: HashMap<&str, &str> =
                                    owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                                let (text, ok) = this.run_node(&wf_id, &node, &vars, &run_id, &cancel).await;
                                (node.id.clone(), text, ok)
                            });
                        }
                    }
                    started
                }};
            }

            for started in [schedule_ready!()] {
                for node in started {
                    yield Ok(WorkflowEvent::NodeStarted { node });
                }
            }
            while let Some((node_id, text, ok)) = inflight.next().await {
                yield Ok(WorkflowEvent::NodeFinished { node: node_id.clone(), ok, output: text.clone() });
                done.insert(node_id.as_str().to_string());
                outputs.insert(node_id.as_str().to_string(), (text, ok));
                if cancel.is_cancelled() {
                    break; // stop scheduling downstream once canceled
                }
                let started = schedule_ready!();
                for node in started {
                    yield Ok(WorkflowEvent::NodeStarted { node });
                }
            }
```

NOTE: this Task ALSO adds `output` to `NodeFinished` (`output: text.clone()`) because the rewrite emits it inline — so the `WorkflowEvent::NodeFinished` variant must gain `output: String` HERE (it can't compile otherwise). That means Phase B's event change folds into THIS commit for the executor side; the SINK/producer side of Phase B (the `Result` change) is a SEPARATE task (Task 3). To keep Task 1 self-contained-green, also update the executor's OWN tests (the `#[cfg(test)] mod tests`) that match `NodeFinished{node,ok}` → `NodeFinished{node,ok,output}` (add `..` or bind `output`), and the streaming producer + sink consumers must tolerate the new field. **However** the `SseSink`/`drain_workflow` in `bridge-a2a-inbound` match `NodeFinished{node,ok}` — adding a field breaks them → they must be updated in the SAME commit (the ripple). So Task 1 = the executor rewrite + `output` field + ALL consumers updated to the new variant shape (but sink methods still return `()` — the `Result` change is Task 3). Concretely, update `drain_workflow`'s match arm and `SseSink::node_finished` call sites to the 3-field variant.

- [ ] **Step 3: Update the variant + all consumers in this commit.** Change `WorkflowEvent::NodeFinished` to `{ node: NodeId, ok: bool, output: String }`. Update: executor tests; `crates/bridge-a2a-inbound/src/workflow_sink.rs` `drain_workflow`'s `Ok(WorkflowEvent::NodeFinished { node, ok }) =>` arm to `{ node, ok, output }` (pass `output` is not yet used by `node_finished` — keep `node_finished(node.as_str(), ok)` for now, ignore `output` with `let _ = output;` or `..`); any other match on `NodeFinished`.

- [ ] **Step 4: Verify behavior preserved.** Run:
  `cargo test -p bridge-workflow` (executor tests green — fan-out still produces the same terminal output; pipeline order preserved by dependency gating)
  `cargo test -p bridge-a2a-inbound --test workflow_producer` (ALL streaming tests green, esp. `streaming_workflow_emits_node_status_synth_artifact_and_completed`)
  `cargo clippy -p bridge-workflow -p bridge-a2a-inbound --all-targets -- -D warnings`
  Expected: all green. If a streaming test asserts a STRICT order of node-status frames that completion-driven changes, confirm the assertion is about presence/terminal (not strict sibling order) — the existing test asserts ≥1 node status + synth artifact + Completed, which holds. If it genuinely over-asserts order, that's a real finding — report it (do not weaken a real ordering guarantee).

- [ ] **Step 5: Add a completion-order test** proving a fast fan-out leg finishes before a slow sibling. In the executor `#[cfg(test)] mod tests`, add a test with two parallel nodes where one backend is fast and one is delayed (use a `tokio::time::sleep` or a Notify in the fake backend), driving `run()` and asserting the fast node's `NodeFinished` is yielded before the slow one's. (Model on the existing executor fakes.)
  Run: `cargo test -p bridge-workflow completion_order` → PASS.

- [ ] **Step 6: Commit**
```bash
git add crates/bridge-workflow/src/executor.rs crates/bridge-a2a-inbound/src/workflow_sink.rs
git commit -m "perf(workflow): completion-driven scheduling (FuturesUnordered) + NodeFinished{output}; behavior-preserving"
```
**No trailer.**

---

## Phase B — Fallible `WorkflowSink` (so a checkpoint-write failure can abort the run)

### Task 2: `WorkflowSink` methods return `Result`; `drain_workflow` aborts on error

**Files:** Modify `crates/bridge-a2a-inbound/src/workflow_sink.rs`, `crates/bridge-a2a-inbound/src/server.rs` (SseSink), and tests.

- [ ] **Step 1: Write the failing test.** Add to `crates/bridge-a2a-inbound/tests/workflow_producer.rs` a test using a sink that errors on `terminal` (or `node_finished`) and asserts `drain_workflow` returns the error / the run is aborted. Since `drain_workflow` is `pub(crate)`, test via the detached runner path instead: a `TaskStore` whose `put_node_checkpoint` errors → the detached run ends `Failed` (this becomes meaningful after Task 11; for Task 2, unit-test the sink/drain contract directly in a `#[cfg(test)] mod` inside `workflow_sink.rs`). Add to `workflow_sink.rs` tests:
```rust
#[cfg(test)]
mod sink_tests {
    use super::*;
    struct FailTerminalSink;
    #[async_trait::async_trait]
    impl WorkflowSink for FailTerminalSink {
        async fn terminal(&mut self, _o: WorkflowOutcome, _out: String) -> Result<(), BridgeError> {
            Err(BridgeError::StoreFailure)
        }
    }
    // drain over a one-node stream that yields Terminal → drain returns Err.
    #[tokio::test]
    async fn drain_aborts_on_sink_error() {
        let stream: WorkflowStream = Box::pin(futures::stream::iter(vec![Ok(
            WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output: "x".into() },
        )]));
        let mut sink = FailTerminalSink;
        assert!(drain_workflow(stream, &mut sink).await.is_err());
    }
}
```

- [ ] **Step 2: Run → fails** (methods return `()`, drain returns `bool`).

- [ ] **Step 3: Change the contract.** In `workflow_sink.rs`:
  - `WorkflowSink` methods → `async fn node_started(&mut self, _node: &str) -> Result<(), BridgeError> { Ok(()) }`, same for `node_finished(&mut self, _node, _ok)`, `terminal(&mut self, outcome, output)` (required), `error(&mut self, _err)`.
  - `drain_workflow<S: WorkflowSink>(mut stream, sink: &mut S) -> Result<bool, BridgeError>`: in the loop, `sink.node_started(...).await?;` etc.; on `Err(e)` event still call `sink.error(e).await?;`; return `Ok(terminal_seen)`.
  - `SseSink` impls return `Ok(())` (its sends are best-effort `let _ = ...; Ok(())`).
  - `TaskStoreSink::terminal` returns `Ok(())` (it just stores the mapping).

- [ ] **Step 4: Update callers.** In `server.rs`:
  - `spawn_workflow_producer`: `let terminal_seen = crate::workflow_sink::drain_workflow(stream, &mut sink).await.unwrap_or(false);` (the SSE sink never errors; on a hypothetical error, treat as no-terminal → the existing no-terminal fallback fires).
  - `spawn_detached_workflow` (the detached runner): `match crate::workflow_sink::drain_workflow(stream, &mut sink).await { Ok(seen) => { /* existing terminal/no-terminal logic */ } Err(_) => { let _ = srv.task_store.set_terminal(&task, Failed, None, Some("checkpoint write failed"), now_ms()).await; } }` then `fin.done = true; remove token`. (The `?`-less handling keeps the finalizer + token cleanup intact.)

- [ ] **Step 5: Run** `cargo test -p bridge-a2a-inbound` + the sink test + `cargo clippy … -D warnings` → green.

- [ ] **Step 6: Commit**
```bash
git add crates/bridge-a2a-inbound/src/workflow_sink.rs crates/bridge-a2a-inbound/src/server.rs
git commit -m "refactor(inbound): WorkflowSink methods return Result; drain_workflow aborts on sink error"
```

---

## Phase C — `WorkflowGraph` serde

### Task 3: derive serde on the graph types

**Files:** `crates/bridge-workflow/src/graph.rs`, `crates/bridge-workflow/Cargo.toml`.

- [ ] **Step 1: Failing test.** Add to graph.rs tests a round-trip:
```rust
    #[test]
    fn graph_serde_roundtrip() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("wf").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("a").unwrap(),
                agent: AgentId::parse("x").unwrap(),
                prompt_template: "t {{input}}".into(),
                inputs: vec![],
            }],
        };
        let s = serde_json::to_string(&g).unwrap();
        let g2: WorkflowGraph = serde_json::from_str(&s).unwrap();
        assert_eq!(g2.nodes.len(), 1);
        assert_eq!(g2.nodes[0].id.as_str(), "a");
    }
```
- [ ] **Step 2: Run → fails** (no serde derive / no serde_json dev-dep).
- [ ] **Step 3: Implement.** Add `serde::{Serialize, Deserialize}` to the `#[derive(...)]` on `WorkflowGraph` AND `WorkflowNode`. In `Cargo.toml`: add `serde = { workspace = true }` under `[dependencies]` and `serde_json = { workspace = true }` under `[dev-dependencies]` (confirm the workspace exposes these; `bridge-core` uses `serde` already).
- [ ] **Step 4: Run** `cargo test -p bridge-workflow graph_serde_roundtrip` + `cargo clippy -p bridge-workflow --all-targets -- -D warnings` → green.
- [ ] **Step 5: Commit** `git commit -m "feat(workflow): serde derive on WorkflowGraph/WorkflowNode (for resume snapshot)"`

---

## Phase D — `TaskStore` port additions + `MemoryTaskStore`

### Task 4: `ResumeClaim` + `TaskRecord` fields + trait methods + `MemoryTaskStore`

**Files:** `crates/bridge-core/src/task_store.rs`.

- [ ] **Step 1: Failing tests.** Append to the task_store tests:
```rust
    #[tokio::test]
    async fn node_checkpoints_roundtrip_and_claim() {
        let s = MemoryTaskStore::new();
        let t = TaskId::parse("t").unwrap();
        // create now carries input + workflow_spec_json
        s.create(&TaskRecord {
            id: t.clone(), workflow: "wf".into(), status: TaskRecordStatus::Working,
            result: None, error: None, created_ms: 1, updated_ms: 1,
            input: "DIFF".into(), workflow_spec_json: Some("{\"v\":1}".into()), resume_attempts: 0,
        }).await.unwrap();
        s.put_node_checkpoint(&t, &NodeId::parse("codex").unwrap(), "OUT", true, 2).await.unwrap();
        let cps = s.node_checkpoints(&t).await.unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(cps[0].1, "OUT");
        // claim increments up to cap then Exhausted
        assert!(matches!(s.claim_resume_attempt(&t, 2, 9).await.unwrap(), ResumeClaim::Resumable { attempt: 1 }));
        assert!(matches!(s.claim_resume_attempt(&t, 2, 9).await.unwrap(), ResumeClaim::Resumable { attempt: 2 }));
        assert!(matches!(s.claim_resume_attempt(&t, 2, 9).await.unwrap(), ResumeClaim::Exhausted));
        let wt = s.working_tasks().await.unwrap();
        assert_eq!(wt.len(), 1);
        assert_eq!(wt[0].input, "DIFF");
    }
```
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** In `task_store.rs`:
  - Add fields to `TaskRecord`: `pub input: String,` `pub workflow_spec_json: Option<String>,` `pub resume_attempts: u32,` (update EVERY `TaskRecord{...}` literal across the codebase + tests — the detached arm, the W3a tests; do this in this commit since it's additive struct fields). To minimize ripple, consider `#[derive(Default)]`-friendly construction is NOT used (literals everywhere) — so every literal must add the 3 fields; grep `TaskRecord {` and update all.
  - Add `pub enum ResumeClaim { Resumable { attempt: u32 }, Exhausted }`.
  - Add a `pub struct WorkingTask { pub id: TaskId, pub workflow: String, pub input: String, pub workflow_spec_json: Option<String>, pub resume_attempts: u32 }`.
  - Add trait methods: `put_node_checkpoint(&self, task: &TaskId, node: &NodeId, output: &str, ok: bool, ts: i64) -> Result<(),BridgeError>`; `node_checkpoints(&self, task: &TaskId) -> Result<Vec<(NodeId,String,bool)>,BridgeError>`; `claim_resume_attempt(&self, task: &TaskId, cap: u32, now_ms: i64) -> Result<ResumeClaim,BridgeError>`; `working_tasks(&self) -> Result<Vec<WorkingTask>,BridgeError>`.
  - Implement all on `MemoryTaskStore` (a second `Mutex<HashMap<(String,String), (String,bool,i64)>>` for checkpoints; `claim_resume_attempt`: lock the task row, if `resume_attempts >= cap` → `Exhausted`, else `resume_attempts += 1; updated_ms = now` (also set a `last_resume_ms` if you carry it — for Memory it can be folded into updated_ms) → `Resumable{attempt: resume_attempts}`; `working_tasks`: rows with status Working).
- [ ] **Step 4: Run** `cargo test -p bridge-core` (the new test + existing) → green; `cargo clippy -p bridge-core --all-targets -- -D warnings`.
- [ ] **Step 5: Build the workspace** to surface every `TaskRecord{...}` literal that now needs the 3 fields: `cargo build --workspace 2>&1 | head`. Fix each literal (detached arm in server.rs; all W3a tests in workflow_producer.rs; sqlite tests). This is the additive-field ripple — do it in this commit.
- [ ] **Step 6: Commit** `git commit -m "feat(core): TaskStore node-checkpoint + claim_resume_attempt + working_tasks; TaskRecord input/snapshot/attempts"`

---

## Phase E — `SqliteStore` migration + impl

### Task 5: `task_node_checkpoints` table, additive `tasks` columns, `PRAGMA foreign_keys=ON`, impls

**Files:** `crates/bridge-store/src/sqlite.rs`.

- [ ] **Step 1: Failing tests.** Append to sqlite tests:
```rust
    #[tokio::test]
    async fn w3b_schema_and_checkpoints() {
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t").unwrap();
        s.create(&TaskRecord { id: t.clone(), workflow: "wf".into(), status: TaskRecordStatus::Working,
            result: None, error: None, created_ms: 1, updated_ms: 1,
            input: "DIFF".into(), workflow_spec_json: Some("{\"v\":1}".into()), resume_attempts: 0 }).await.unwrap();
        s.put_node_checkpoint(&t, &NodeId::parse("codex").unwrap(), "OUT", true, 2).await.unwrap();
        assert_eq!(s.node_checkpoints(&t).await.unwrap()[0].1, "OUT");
        assert!(matches!(s.claim_resume_attempt(&t, 1, 9).await.unwrap(), ResumeClaim::Resumable{attempt:1}));
        assert!(matches!(s.claim_resume_attempt(&t, 1, 9).await.unwrap(), ResumeClaim::Exhausted));
        assert_eq!(s.working_tasks().await.unwrap()[0].input, "DIFF");
    }

    #[tokio::test]
    async fn migration_on_old_schema_db() {
        // open a file DB with only the OLD tasks table, insert a row, reopen with new code → columns added, row intact.
        let dir = std::env::temp_dir().join(format!("a2a-w3b-mig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.db");
        {
            use rusqlite::Connection;
            let c = Connection::open(&path).unwrap();
            c.execute_batch("CREATE TABLE tasks(id TEXT PRIMARY KEY, workflow TEXT NOT NULL, status TEXT NOT NULL, result TEXT, error TEXT, created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL);").unwrap();
            c.execute("INSERT INTO tasks(id,workflow,status,created_ms,updated_ms) VALUES('old','wf','completed',1,1)", []).unwrap();
        }
        let s = SqliteStore::open(&path).unwrap();
        let got = s.get(&TaskId::parse("old").unwrap()).await.unwrap().unwrap();
        assert_eq!(got.status, TaskRecordStatus::Completed);
        assert_eq!(got.input, ""); // default
        drop(s); let _ = std::fs::remove_dir_all(&dir);
    }
```
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** In `sqlite.rs`:
  - In `open`/`open_in_memory`, after opening the connection, `conn.execute_batch("PRAGMA foreign_keys = ON;")` (before `create_schema`), OR set it in `create_schema`. (rusqlite: `PRAGMA foreign_keys=ON` is per-connection.)
  - `create_schema`: append to the `execute_batch` the `task_node_checkpoints` table (per spec §3D) + the index/PK. Then run an **idempotent migration** for the additive `tasks` columns: a helper `fn migrate_tasks_columns(conn)` that reads `PRAGMA table_info(tasks)` into a set and `ALTER TABLE tasks ADD COLUMN …` for any of `input`/`workflow_spec_json`/`resume_attempts`/`last_resume_ms` not present. (CREATE TABLE IF NOT EXISTS won't add columns to an existing table — the ALTER is required for old DBs.)
  - Update `create` to INSERT the new columns (`input`, `workflow_spec_json`, `resume_attempts` default 0); update `row_to_task`'s SELECT + mapping to read them (`input` default '', `workflow_spec_json` nullable, `resume_attempts`).
  - Implement `put_node_checkpoint` (`INSERT OR REPLACE INTO task_node_checkpoints …`), `node_checkpoints` (`SELECT node_id, output, ok FROM task_node_checkpoints WHERE task_id=?`), `claim_resume_attempt` (a transaction: `SELECT resume_attempts`; if `>= cap` → Exhausted; else `UPDATE tasks SET resume_attempts=resume_attempts+1, last_resume_ms=? WHERE id=?` → Resumable{new}), `working_tasks` (`SELECT id, workflow, input, workflow_spec_json, resume_attempts FROM tasks WHERE status='working'`).
- [ ] **Step 4: Run** `cargo test -p bridge-store` + clippy → green.
- [ ] **Step 5: Commit** `git commit -m "feat(store): task_node_checkpoints + tasks migration (input/snapshot/attempts) + foreign_keys ON"`

---

## Phase F — executor `run_from(seed)`

### Task 6: `run_from` seed entry + seed validation

**Files:** `crates/bridge-workflow/src/executor.rs`.

- [ ] **Step 1: Failing tests.** In the executor tests: a 3-node graph (a,b → c); call a new `run_from` seeded with `{a:(…,true), b:(…,true)}` and assert only `c` runs (the fakes record which nodes were prompted); a seed with an unknown node id → the stream yields an error/terminal Failed; a seed missing a non-root node's input (closure violation) → error.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** Refactor `run()` into `run_from(graph, input, run_id, cancel, seed: HashMap<NodeId,(String,bool)>)`; `run()` calls `run_from(graph, input, run_id, cancel, HashMap::new())`. At the top of the stream body, BEFORE the loop: validate the seed — for each seeded `NodeId`, it must exist in `graph.nodes` (else `yield Err(BridgeError::ConfigInvalid{reason:"resume seed: unknown node"})` then return); for each seeded non-root node, all its `inputs` must be seeded (else `yield Err(...closure...)`; return). Then initialize `done`+`outputs` from the seed (`done = seed.keys().map(as_str)`, `outputs = seed mapped to (text,ok)`), and the completion-driven `schedule_ready!` naturally skips seeded nodes (they're in `done`). Keep `run_id` as passed (boot supplies `"{task}-resume-{n}"`).
- [ ] **Step 4: Run** `cargo test -p bridge-workflow` (incl. the new + the W1/existing) + clippy → green.
- [ ] **Step 5: Commit** `git commit -m "feat(workflow): run_from(seed) resume entry + seed validation (run = empty seed)"`

---

## Phase G — capture in the runner + persist input/snapshot at submit

### Task 7: `TaskStoreSink` persists a checkpoint on each `NodeFinished`

**Files:** `crates/bridge-a2a-inbound/src/workflow_sink.rs` (+ how the runner wires the sink), tests in workflow_producer.rs.

- [ ] **Step 1: Failing test.** In workflow_producer.rs: drive `spawn_detached_workflow_for_test` over a multi-node workflow with a real `MemoryTaskStore`; after the run, assert `store.node_checkpoints(task)` has a row per node with the right outputs. Also: a store whose `put_node_checkpoint` errors → the task ends `Failed` (the Phase B abort path).
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** Give `TaskStoreSink` access to the store + task id + a `now` clock: change its construction to `TaskStoreSink::new(store: Arc<dyn TaskStore>, task: TaskId)` and implement `node_finished(&mut self, node, ok)` — wait, the sink's `node_finished` doesn't receive `output`. The `output` is on the EVENT; `drain_workflow` passes `node`+`ok` to `node_finished` but the new `NodeFinished{output}` means `drain_workflow` should pass `output` to `node_finished` too. **Update the `WorkflowSink::node_finished` signature to `node_finished(&mut self, node: &str, ok: bool, output: &str) -> Result<(),BridgeError>`** and `drain_workflow` to pass `output`. Then `TaskStoreSink::node_finished` calls `self.store.put_node_checkpoint(&self.task, &NodeId::parse(node)?, output, ok, now_ms()).await?`. `SseSink::node_finished` ignores `output`. Update the detached runner to build `TaskStoreSink::new(srv.task_store.clone(), task.clone())` instead of the output-capturing-only sink (the terminal mapping it still captures via `terminal`).
- [ ] **Step 4: Run** the new tests + `cargo test -p bridge-a2a-inbound` + clippy → green.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): TaskStoreSink checkpoints each finished node; write-failure fails the task"`

### Task 8: persist `input` + versioned `workflow_spec_json` at submit

**Files:** `crates/bridge-a2a-inbound/src/server.rs` (the detached arm), tests.

- [ ] **Step 1: Failing test.** Submit a workflow via the unary detached arm (the existing `unary_workflow_send_returns_working_task` harness) and assert the persisted `TaskRecord` has `input == "DIFF"` and `workflow_spec_json` is `Some(...)` containing `"v":1`.
- [ ] **Step 2: Run → fails** (the arm doesn't set input/snapshot yet).
- [ ] **Step 3: Implement.** In the `RouteTarget::Workflow(ref wf_id)` detached arm: build the input string from parts; serialize the resolved graph: `let snap = srv.workflows.get(wf_id).map(|g| serde_json::json!({"v":1,"graph": &**g}).to_string());` (the graph is `Arc<WorkflowGraph>`, now serde). Set `TaskRecord{ … input: text, workflow_spec_json: snap, resume_attempts: 0 }` in `create`.
- [ ] **Step 4: Run** + clippy → green.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): persist submit input + versioned workflow_spec_json snapshot at detached create"`

---

## Phase H — boot resume routine on `InboundServer`

### Task 9: generalize `spawn_detached_workflow` to accept an injected graph

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1:** Change `spawn_detached_workflow`'s signature from `(srv, task, text_parts, wf_id, token)` to **`(srv, task, text_parts: Vec<String>, graph: Arc<WorkflowGraph>, run_id: String, token)`** — it no longer resolves the graph from `wf_id` (the caller injects it) and uses the given `run_id`. Internally call `run_from(graph, input_from(text_parts), run_id, cancel, HashMap::new())` (fresh submit = empty seed). Keep the finalizer + token + terminal-write logic. The unary detached arm passes `srv.workflows.get(wf_id).cloned()` + `run_id = task.as_str().to_string()`. Update `spawn_detached_workflow_for_test`/`_with_token_for_test` to resolve the graph from the test server's `workflows` map and pass `run_id = task` (so existing Task-7/Phase-G tests still pass). *(Task 10 then adds a trailing `seed: HashMap<NodeId,(String,bool)>` param — final shape `(srv, task, text_parts, graph, run_id, token, seed)` — with the fresh-submit callers passing an empty seed.)*
- [ ] **Step 2:** `cargo test -p bridge-a2a-inbound` → green (refactor, no behavior change). Commit `git commit -m "refactor(inbound): spawn_detached_workflow takes an injected graph + run_id (for resume)"`

### Task 10: the boot resume routine + serve wiring + `resume_attempt_cap` config

**Files:** `crates/bridge-a2a-inbound/src/server.rs`, `bin/a2a-bridge/src/{config.rs,main.rs}`, tests.

- [ ] **Step 1: Failing test.** In workflow_producer.rs: seed a `MemoryTaskStore` with a `Working` task (input + a `workflow_spec_json` snapshot of `review_graph`) + a checkpoint for `codex` only; build the server `.with_task_store` + `.with_workflows`; call a new `srv.resume_working_tasks(cap).await`; await; assert `codex` was NOT re-prompted, `claude`+`synth` ran, and `tasks/get` → Completed. Also tests: a `Working` row with `workflow_spec_json = None` → Interrupted; resume_attempts at cap → Interrupted; a task whose terminal node already has a checkpoint → finalized without prompting.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement `InboundServer::resume_working_tasks(&self, cap: u32) -> ()`** (an `async fn` on `InboundServer`, callable from serve). For each `working_tasks()`:
  - `workflow_spec_json` None → `set_terminal(Interrupted, "not resumable")`; continue.
  - parse the envelope `{"v":1,"graph":…}`; unknown version / parse error → `set_terminal(Interrupted, "unreadable workflow snapshot")`; continue.
  - load `node_checkpoints`; find the terminal node id from the graph; if a checkpoint exists for it → finalize directly: ok=true → `set_terminal(Completed, output)`; ok=false → `set_terminal(Failed, …, output)`; continue (no attempt consumed).
  - else `claim_resume_attempt(task, cap, now_ms())`: `Exhausted` → `set_terminal(Interrupted, "resume attempt cap exceeded")`; `Resumable{attempt}` → seed = node_checkpoints mapped to `HashMap<NodeId,(String,bool)>`; register a `CancellationToken` in `workflow_cancels`; `spawn_detached_workflow(self_arc, task, /*input*/ wt.input, graph, format!("{}-resume-{}", task, attempt), token, seed)`. (Generalize the spawn signature from Task 9 to also accept `seed` + `input` — the fresh-submit path passes empty seed + the request input.)
  - NOTE: `resume_working_tasks` needs `&Arc<InboundServer>` (to clone into the spawned runner). Make it a free fn `pub async fn resume_working_tasks(srv: &Arc<InboundServer>, cap: u32)` mirroring `spawn_detached_workflow`'s `&Arc<InboundServer>` pattern.
- [ ] **Step 4: Config + serve wiring.** `config.rs`: add `pub resume_attempt_cap: Option<u32>` to `StoreConfig` (default via `.unwrap_or(3)`). `main.rs` serve: after building the file-backed task store, instead of `sweep_interrupted`, build the `InboundServer`, then `resume_working_tasks(&server, cap).await` BEFORE binding the listener. (Keep `sweep_interrupted` for the in-memory/no-path branch, or drop it — for the memory branch there's nothing durable to resume.) Confirm the boot order: open store → build InboundServer → resume → bind → serve.
- [ ] **Step 5: Run** the boot tests + `cargo test --workspace` + clippy → green.
- [ ] **Step 6: Commit** `git commit -m "feat(inbound,bin): boot resume routine on InboundServer (replaces sweep for workflow tasks) + resume_attempt_cap"`

### Task 11: poison-cap deterministic test

- [ ] **Step 1:** A test that `claim_resume_attempt`s a task to the cap (without completing it) across simulated boots, then asserts `resume_working_tasks` marks it `Interrupted` (no infinite loop). Run → PASS. Commit `git commit -m "test(inbound): poison-task resume cap terminates at Interrupted"`

---

## Phase I — verification, live gate, ADR

### Task 12: full sweep + coverage
- [ ] `cargo fmt --all && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` → all green.
- [ ] `cargo llvm-cov clean --workspace` then `--workspace --fail-under-lines 85`, `--package bridge-core --fail-under-lines 90`, `--package bridge-workflow --fail-under-lines 90`. Top up tests if any floor dips. Commit any additions.

### Task 13: gated live (real agents)
- [ ] Build; create `/tmp/w3b-serve/a2a-bridge.toml` (codex+claude agents, the `code-review` workflow with ABSOLUTE prompt paths, `[server] addr`, `[store] path=/tmp/w3b-tasks.db`). `cd /tmp/w3b-serve && serve &`. `submit code-review --input <diff>`; tail the agent_stderr/checkpoint log; once ONE reviewer's checkpoint is written (poll `task get` or the DB), `kill` serve; restart; verify (via `task_node_checkpoints` timestamps or agent invocation logs) the checkpointed reviewer is NOT re-prompted, the other reviewer + synth run, and `task get` → `Completed`. Record in the ADR.

### Task 14: ADR-0011
- [ ] Write `docs/adr/0011-crash-resume.md`: the decision (auto crash-resume from node checkpoints); the components (completion-driven scheduling; additive NodeFinished{output}+fallible sink; run_from seed; lean task_node_checkpoints + versioned snapshot; boot resume on InboundServer + cap; terminal-checkpoint short-circuit narrowing the W3a write-failure gap); the dual-design+dual-review provenance + the adopt-completion-driven decision; the live-gate result; follow-ons (streaming reattach, retry policy, history query API, retention/prune). Commit with the controller trailer.

---

## DoD §9 → tasks
| DoD | Task |
|-----|------|
| 1 (completion-driven; streaming green; order/write-ahead tests) | 1 |
| 2 (NodeFinished{output}; sink Result; drain aborts) | 1 (event), 2 (sink) |
| 3 (run_from seed; unknown/closure rejection) | 6 |
| 4 (WorkflowGraph serde; unparseable handled) | 3 (serde), 10 (unparseable→Interrupted) |
| 5 (checkpoints table + migration + foreign_keys) | 5 |
| 6 (TaskStore methods + create persists; both impls) | 4, 5 |
| 7 (runner checkpoints; write-failure fails task) | 7 |
| 8 (boot resume; short-circuit; Interrupted cases; replaces sweep; token before spawn) | 10 |
| 9 (resume_attempt_cap config; poison cap) | 10, 11 |
| 10 (gated live) | 13 |
| 11 (fmt/clippy/coverage) | 12 |

## Notes for the implementer
- **Riskiest = Task 1** (completion-driven rewrite). It folds the `NodeFinished{output}` field in (can't compile otherwise) + updates all consumers. Land it green-isolated; the W1 streaming test is the oracle. If the `FuturesUnordered` + `schedule_ready!` macro borrow-checks awkwardly (the `&this` capture, the `owned` move), prefer a small helper fn over the macro — but the existing `run()` already does exactly this capture pattern with `join_all`, so it borrow-checks.
- **Additive `TaskRecord` fields (Task 4)** ripple to every `TaskRecord{...}` literal — grep and update all in that commit (the build surfaces them).
- **Sink `node_finished` gains `output`** (Task 7) — a second small signature change after Task 2; sequence it as written.
- Firewall: `~/code/a2a-local-bridge` black-box only. `serve` reads a fixed `a2a-bridge.toml` from CWD. workflow_producer tests run under `-p bridge-a2a-inbound`. Controller docs (this plan, ADR-0011) carry the `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer; task commits do NOT.
