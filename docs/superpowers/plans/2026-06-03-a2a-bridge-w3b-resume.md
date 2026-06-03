# W3b — Node checkpoint history + crash-resume Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A detached workflow that is `Working` when `serve` restarts resumes from per-node checkpoints — re-running only not-yet-finished nodes, reusing finished nodes' outputs — instead of being swept to `Interrupted`.

**Architecture:** Completion-driven executor scheduling (so a fan-out leg is checkpointable the instant it finishes) + an additive `NodeFinished{output}` over a now-fallible `WorkflowSink` + a `run_from(seed)` resume entry (executor stays pure) + a lean `task_node_checkpoints` table & a versioned graph snapshot in `tasks` + an auto-resume boot routine on `InboundServer` with an attempt cap.

**Tech Stack:** Rust, tokio, `futures::stream::FuturesUnordered`, rusqlite, serde, the existing `TaskStore`/`WorkflowExecutor`/detached-runner from W3a.

**Spec:** `docs/superpowers/specs/2026-06-03-a2a-bridge-w3b-resume-design.md` (rev1, dual-designed + dual-reviewed).

**Branch:** `feat/w3b-resume` off `main`.

**Plan rev2 (post dual-review):** Codex (executability) + Claude (architecture) reviewed rev1. Both confirmed the spine + phase ordering + the Task-1 borrow-checker feasibility + the migration SQL. Folded: split Task 1 → 1a/1b (additive field first, scheduler isolated second); the cancel **drain-not-break** fix + test; the DoD-1 write-ahead barrier test; Task 4 `SqliteStore` stubs (green-per-task); the `bin/.../main.rs` consumer + `cargo check --workspace`; one settled `spawn_detached_workflow`/`resume_working_tasks` shape; plain `INSERT` for checkpoints + cascade/foreign_keys migration test; split Task 10 → 10a/10b; boot-test determinism; unknown-version test; dropped the redundant `WorkingTask` struct (`working_tasks() -> Vec<TaskRecord>`).

**Grounding facts (confirmed against the code):**
- Executor `run()` (executor.rs ~129-176) builds `outputs: HashMap<String,(String,bool)>` + `done: HashSet<String>`, loops `while done.len() < graph.nodes.len()`, filters `ready` (not done + all inputs done), yields `NodeStarted` per ready node, runs them via `futures::future::join_all`, then per result yields `NodeFinished{node,ok}` + `done.insert` + `outputs.insert`. Terminal = `outputs[terminal_id]`. `run_node(&self, wf_id, node, vars:&HashMap<&str,&str>, run_id, cancel) -> (String,bool)`; its cancel branch runs `backend.cancel()` (executor.rs:109) + `forget_session()` (executor.rs:125).
- `WorkflowEvent::NodeFinished{node: NodeId, ok: bool}` (NO output today). `WorkflowSink` trait methods return `()`; `drain_workflow(stream, sink) -> bool` (terminal_seen). `SseSink`, `TaskStoreSink`(+`take()`), `Finalizer`, `now_ms()` in workflow_sink.rs.
- `WorkflowGraph`/`WorkflowNode` derive `Debug, Clone` ONLY (graph.rs); `NodeId`/`WorkflowId`/`AgentId` derive serde (ids.rs:7/33). `bridge-workflow/Cargo.toml` has no `serde` dep.
- `TaskStore` (task_store.rs) has create/set_terminal/get/list/sweep_interrupted/cancel_if_working; `TaskRecord{id,workflow,status,result,error,created_ms,updated_ms}`. `SqliteStore` `tasks` table + `create_schema` (execute_batch); `open(path)` holds an `fs2` lock; `MemoryTaskStore` is the in-mem default. `BridgeError::{ConfigInvalid,StoreFailure}` exist.
- The detached arm (server.rs `unary_message` `RouteTarget::Workflow(ref wf_id)`) creates `TaskRecord{Working}` + registers a token + `spawn_detached_workflow(&srv, task, text_parts, wf_id.clone(), token)` + returns `a2a::Task{Working}`. `spawn_detached_workflow(srv,task,text_parts,wf_id,token)->JoinHandle`. `InboundServer` fields incl. `executor: Option<Arc<WorkflowExecutor>>`, `workflows: Arc<HashMap<WorkflowId,Arc<WorkflowGraph>>>`, `task_store: Arc<dyn TaskStore>`, `workflow_cancels: Arc<Mutex<HashMap<TaskId,CancellationToken>>>`.
- `NodeFinished` is consumed in: executor tests; `workflow_sink.rs` `drain_workflow`; **`bin/a2a-bridge/src/main.rs` (~line 241, the CLI run-workflow printer)** — all three must be updated when the variant widens.
- serve (main.rs) opens the store (file→`SqliteStore::open`+sweep, else Memory), `.with_task_store`, binds. `StoreConfig{path}`. Tests: workflow_producer.rs under `-p bridge-a2a-inbound`; `-p a2a-bridge` has no `--lib`. The W1 streaming guard test: `streaming_workflow_emits_node_status_synth_artifact_and_completed` (workflow_producer.rs:354); a poll-to-terminal helper pattern lives at workflow_producer.rs:1104.

---

## File Structure
- **Modify** `crates/bridge-workflow/src/executor.rs` — `NodeFinished{output}`; completion-driven scheduling; `run_from(seed)`.
- **Modify** `crates/bridge-workflow/src/graph.rs` + `Cargo.toml` — serde derives + dep.
- **Modify** `crates/bridge-a2a-inbound/src/workflow_sink.rs` — fallible sink + `drain_workflow` abort; `TaskStoreSink` checkpoints.
- **Modify** `crates/bridge-a2a-inbound/src/server.rs` — `SseSink`/producer for new sigs; `spawn_detached_workflow` injected-graph+seed; the boot `resume_working_tasks` free fn; persist input+snapshot in the detached arm.
- **Modify** `crates/bridge-core/src/task_store.rs` — `TaskStore` methods + `ResumeClaim` + `TaskRecord` fields + `MemoryTaskStore`.
- **Modify** `crates/bridge-store/src/sqlite.rs` — stubs (Task 4) then migration + `task_node_checkpoints` + impls + `PRAGMA foreign_keys=ON` (Task 5).
- **Modify** `bin/a2a-bridge/src/main.rs` — the `NodeFinished` consumer (Task 1a); serve resume wiring (Task 10b).
- **Modify** `bin/a2a-bridge/src/config.rs` — `resume_attempt_cap` (Task 10b).
- **Modify** tests in `crates/bridge-workflow/src/executor.rs` (tests mod) + `crates/bridge-a2a-inbound/tests/workflow_producer.rs`.
- **Create** `docs/adr/0011-crash-resume.md`.

---

## Phase A — Completion-driven scheduling (ISOLATED, behavior-preserving FIRST)

> **Decomposition note (dual-review):** rev1 folded the `NodeFinished{output}` field INTO the `FuturesUnordered` rewrite. The Claude review observed the field is INDEPENDENT of the scheduler — the existing `join_all` path already has the node `text` in scope (executor.rs:164-167) — so the additive field lands FIRST over `join_all` (Task 1a, a pure ripple), then the scheduler rewrite lands TRULY isolated (Task 1b). This honors the spec's "isolated first commit" intent better than folding them.

### Task 1a: widen `NodeFinished` to `{node, ok, output}` over the existing `join_all` (behavior-preserving ripple)

**Files:** Modify `crates/bridge-workflow/src/executor.rs` (the `WorkflowEvent` enum + the `run()` `NodeFinished` yield), `crates/bridge-a2a-inbound/src/workflow_sink.rs` (`drain_workflow` match arm), `bin/a2a-bridge/src/main.rs` (the CLI `NodeFinished` consumer), and executor tests.

**Goal:** add `output: String` to `WorkflowEvent::NodeFinished` and emit the node's text from the EXISTING `join_all` path — NO scheduler change. Pure additive-field ripple (the API-backend/W1 lesson: land the ripple atomically, across the whole workspace).

- [ ] **Step 1: Confirm the regression guard is green BEFORE the change.**
  Run: `cargo test -p bridge-a2a-inbound --test workflow_producer streaming_workflow_emits_node_status_synth_artifact_and_completed`
  Expected: PASS (this is the behavior the change must preserve).

- [ ] **Step 2: Widen the variant + emit `output`.** Change `WorkflowEvent::NodeFinished` to `{ node: NodeId, ok: bool, output: String }`. In the existing `join_all` result loop, the per-result `(text, ok)` is already in scope — yield `NodeFinished { node: node_id.clone(), ok, output: text.clone() }` BEFORE `outputs.insert(...)` (so `text` is still owned). Do NOT change scheduling (still `join_all`).

- [ ] **Step 3: Ripple ALL consumers in this commit.** `grep -rn "NodeFinished" crates bin` and update every match:
  - executor `#[cfg(test)] mod tests` — `NodeFinished{node,ok}` → `{node,ok,output}` (bind or `..`).
  - `crates/bridge-a2a-inbound/src/workflow_sink.rs` `drain_workflow`'s `Ok(WorkflowEvent::NodeFinished { node, ok }) =>` arm → `{ node, ok, output }`; keep `sink.node_finished(node.as_str(), ok)` (sink sig unchanged this task) and `let _ = output;`.
  - `bin/a2a-bridge/src/main.rs` (~line 241) — the CLI's `NodeFinished` match arm. **A real consumer rev1 missed (Codex review) — add `output` to the pattern.**
  - any other match the grep surfaces.

- [ ] **Step 4: Verify across the WHOLE workspace.**
  Run: `cargo check --workspace` (catches every consumer — Codex review), then
  `cargo test -p bridge-workflow`,
  `cargo test -p bridge-a2a-inbound --test workflow_producer`,
  `cargo clippy --workspace --all-targets -- -D warnings`.
  Expected: all green (only the added field; no behavior change).

- [ ] **Step 5: Commit**
```bash
git add -A
git commit -m "feat(workflow): add output to NodeFinished{node,ok,output} over join_all (behavior-preserving ripple)"
```
**No trailer.**

### Task 1b: replace `join_all` with `FuturesUnordered` completion-driven scheduling

**Files:** Modify `crates/bridge-workflow/src/executor.rs` (the `run()` stream body) + executor tests; add a write-ahead test in `crates/bridge-a2a-inbound/tests/workflow_producer.rs`.

**Goal:** schedule the whole DAG with one in-flight set: push each ready node's future into a `FuturesUnordered`; as each completes, yield its `NodeFinished`, update `done`+`outputs`, and push any newly-ready nodes — instead of batch-`join_all`. Preserve: the write-ahead barrier (the `yield NodeFinished` happens before scheduling dependents, and `drain_workflow` awaits the sink between yields — Phase B), the **cancel cleanup** (every in-flight node must still observe the token and run `backend.cancel()`+`forget_session()` — Step 3), the no-terminal/degradation semantics, and the final result. *(Codex confirmed this borrow-checks: the owned `this` clone + `&this` capture compiles because `AgentRegistry: Send+Sync` makes `&WorkflowExecutor` futures `Send`.)*

- [ ] **Step 1: Confirm the guard is green.**
  Run: `cargo test -p bridge-a2a-inbound --test workflow_producer streaming_workflow_emits_node_status_synth_artifact_and_completed` → PASS.

- [ ] **Step 2: Rewrite the `run()` loop body.** Replace the `while done.len() < graph.nodes.len() { … join_all … }` block with the completion-driven version. Keep the surrounding `Box::pin(async_stream::stream! { … })`, the `outputs`/`done` declarations, `terminal_id`, and the terminal computation after the loop. (Ensure `futures::StreamExt` is in scope for `inflight.next()` — add `use futures::StreamExt;` if not already imported in executor.rs.)

```rust
            use futures::stream::FuturesUnordered;
            let mut inflight: FuturesUnordered<_> = FuturesUnordered::new();
            let mut scheduled: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut stop_scheduling = false; // set on cancel: drain in-flight, schedule nothing new

            // Push every not-done/not-scheduled node whose inputs are all done.
            // Returns the node ids newly scheduled (so the caller can emit NodeStarted).
            macro_rules! schedule_ready {
                () => {{
                    let mut started: Vec<NodeId> = Vec::new();
                    if !stop_scheduling {
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
                    }
                    started
                }};
            }

            for node in schedule_ready!() {
                yield Ok(WorkflowEvent::NodeStarted { node });
            }
            while let Some((node_id, text, ok)) = inflight.next().await {
                yield Ok(WorkflowEvent::NodeFinished { node: node_id.clone(), ok, output: text.clone() });
                done.insert(node_id.as_str().to_string());
                outputs.insert(node_id.as_str().to_string(), (text, ok));
                if cancel.is_cancelled() {
                    // Stop scheduling NEW nodes, but keep draining so every already-in-flight
                    // sibling completes its run_node cancel branch (backend.cancel() +
                    // forget_session()). Do NOT `break` — that drops in-flight futures
                    // mid-cleanup → stranded ACP sessions (dual-review blocker).
                    stop_scheduling = true;
                    continue;
                }
                for node in schedule_ready!() {
                    yield Ok(WorkflowEvent::NodeStarted { node });
                }
            }
```

  **Why drain-not-break:** the old `join_all` awaited EVERY sibling in a ready batch, so each observed the cancel token and ran its cancel/forget cleanup (executor.rs:109-125). A `break` after the first post-cancel completion drops the remaining `FuturesUnordered` futures, aborting their cleanup → a stranded ACP session per in-flight leg. Setting `stop_scheduling` and continuing to drain `inflight.next()` preserves the old cleanup guarantee. Siblings finishing after cancel still yield `NodeFinished` (harmless — the runner is tearing down) and never schedule downstream because `stop_scheduling` gates `schedule_ready!`.

- [ ] **Step 3: Cancel-cleanup regression test (blocker fix — both reviewers).** In the executor `#[cfg(test)] mod tests`, add a 2-leg fan-out test where BOTH legs use a fake backend whose `cancel()`/`forget_session()` is observable (increment a shared `Arc<AtomicUsize>`), and whose `prompt` awaits a `Notify` so both legs are genuinely in-flight when the token is cancelled. Drive `run()` with a pre-cancellable token; release the notify, cancel after both legs are in-flight; assert BOTH legs' cleanup ran (count == 2). FAILS under a `break` (one leg dropped mid-cleanup), PASSES under the drain.
  Run: `cargo test -p bridge-workflow cancel_drains_inflight` → PASS.

- [ ] **Step 4: Completion-order test.** Two parallel nodes, one fast backend + one delayed (`tokio::time::sleep`/`Notify` in the fake); drive `run()` and assert the fast node's `NodeFinished` is yielded BEFORE the slow one's (the behavior `join_all` did NOT give).
  Run: `cargo test -p bridge-workflow completion_order` → PASS.

- [ ] **Step 5: Write-ahead barrier test (DoD-1 — both reviewers flagged unauthored).** Prove a downstream node is NOT prompted before its upstream's `NodeFinished` is handled. Drive a 2-node pipeline (a→b) through `drain_workflow` with a sink that, on `node_finished("a", …)`, snapshots the set of nodes the fake backend has been prompted with so far and asserts `b` is NOT among them yet (the barrier holds because `async_stream`'s `yield` suspends the executor until `drain_workflow` awaits the sink, and `b`'s future is only pushed AFTER the yield returns).
  Run: `cargo test -p bridge-a2a-inbound --test workflow_producer write_ahead_barrier` → PASS.

- [ ] **Step 6: Verify behavior preserved.**
  Run: `cargo check --workspace`,
  `cargo test -p bridge-workflow`,
  `cargo test -p bridge-a2a-inbound --test workflow_producer` (esp. the streaming guard),
  `cargo clippy --workspace --all-targets -- -D warnings`.
  Expected: all green. If a streaming test asserts a STRICT sibling order that completion-driven changes, confirm it asserts presence/terminal (the existing test asserts ≥1 node status + synth artifact + Completed, which holds). If it genuinely over-asserts order, report it — do not weaken a real ordering guarantee.

- [ ] **Step 7: Commit**
```bash
git add crates/bridge-workflow/src/executor.rs crates/bridge-a2a-inbound/tests/workflow_producer.rs
git commit -m "perf(workflow): completion-driven scheduling (FuturesUnordered); drain in-flight on cancel"
```
**No trailer.**

---

## Phase B — Fallible `WorkflowSink` (so a checkpoint-write failure can abort the run)

### Task 2: `WorkflowSink` methods return `Result`; `drain_workflow` aborts on error

**Files:** Modify `crates/bridge-a2a-inbound/src/workflow_sink.rs`, `crates/bridge-a2a-inbound/src/server.rs` (SseSink), and tests.

- [ ] **Step 1: Write the failing test.** Add to `workflow_sink.rs` a `#[cfg(test)] mod` test (drain is `pub(crate)` — an in-crate mod can call it):
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
  - `WorkflowSink` methods → `async fn node_started(&mut self, _node: &str) -> Result<(), BridgeError> { Ok(()) }`, same default for `node_finished(&mut self, _node: &str, _ok: bool)`, `error(&mut self, _err: BridgeError)`; `terminal(&mut self, outcome, output)` required (no default).
  - `drain_workflow<S: WorkflowSink>(mut stream, sink: &mut S) -> Result<bool, BridgeError>`: in the loop, `sink.node_started(...).await?;` etc.; on an `Err(e)` stream item still call `sink.error(e).await?;`; return `Ok(terminal_seen)`.
  - `SseSink` impls return `Ok(())` (sends are best-effort `let _ = ...; Ok(())`).
  - `TaskStoreSink::terminal` returns `Ok(())` (it just stores the mapping).

- [ ] **Step 4: Update callers.** In `server.rs`:
  - `spawn_workflow_producer`: `let terminal_seen = crate::workflow_sink::drain_workflow(stream, &mut sink).await.unwrap_or(false);` (the SSE sink never errors; a hypothetical error → no-terminal → the existing no-terminal fallback fires).
  - `spawn_detached_workflow` (the detached runner): `match crate::workflow_sink::drain_workflow(stream, &mut sink).await { Ok(seen) => { /* existing terminal/no-terminal logic */ } Err(_) => { let _ = srv.task_store.set_terminal(&task, Failed, None, Some("checkpoint write failed"), now_ms()).await; } }` then `fin.done = true; remove token`. (`?`-less handling keeps the finalizer + token cleanup intact.)

- [ ] **Step 5: Run** `cargo test -p bridge-a2a-inbound` + the sink test + `cargo clippy --workspace --all-targets -- -D warnings` → green.

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
- [ ] **Step 3: Implement.** Add `serde::{Serialize, Deserialize}` to the `#[derive(...)]` on `WorkflowGraph` AND `WorkflowNode`. In `Cargo.toml`: add `serde = { workspace = true }` under `[dependencies]` and `serde_json = { workspace = true }` under `[dev-dependencies]` (the workspace exposes both; `bridge-core` already uses `serde`).
- [ ] **Step 4: Run** `cargo test -p bridge-workflow graph_serde_roundtrip` + `cargo clippy -p bridge-workflow --all-targets -- -D warnings` → green.
- [ ] **Step 5: Commit** `git commit -m "feat(workflow): serde derive on WorkflowGraph/WorkflowNode (for resume snapshot)"`

---

## Phase D — `TaskStore` port additions + `MemoryTaskStore`

### Task 4: `ResumeClaim` + `TaskRecord` fields + trait methods + `MemoryTaskStore` (+ `SqliteStore` stubs)

**Files:** `crates/bridge-core/src/task_store.rs`; `crates/bridge-store/src/sqlite.rs` (compile-stubs only — real impls in Task 5).

> **Green-per-task fix (Codex review):** adding trait methods without defaults breaks the concrete `SqliteStore` impl, so `cargo build --workspace` would fail at the end of this task. To keep the tree green, this commit ALSO adds throwaway `SqliteStore` stub impls that return `Err(BridgeError::StoreFailure)` (with a `// TODO(Task 5)` marker). Task 5 replaces them with the real SQLite impls + migration. The Task 4 test only exercises `MemoryTaskStore`, so the stubs are not asserted until Task 5.

- [ ] **Step 1: Failing tests.** Append to the task_store tests:
```rust
    #[tokio::test]
    async fn node_checkpoints_roundtrip_and_claim() {
        let s = MemoryTaskStore::new();
        let t = TaskId::parse("t").unwrap();
        // create now carries input + workflow_spec_json + resume_attempts
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
- [ ] **Step 3: Implement (bridge-core).** In `task_store.rs`:
  - Add fields to `TaskRecord`: `pub input: String,` `pub workflow_spec_json: Option<String>,` `pub resume_attempts: u32,`.
  - Add `pub enum ResumeClaim { Resumable { attempt: u32 }, Exhausted }`.
  - Add trait methods: `put_node_checkpoint(&self, task: &TaskId, node: &NodeId, output: &str, ok: bool, ts: i64) -> Result<(),BridgeError>`; `node_checkpoints(&self, task: &TaskId) -> Result<Vec<(NodeId,String,bool)>,BridgeError>`; `claim_resume_attempt(&self, task: &TaskId, cap: u32, now_ms: i64) -> Result<ResumeClaim,BridgeError>`; `working_tasks(&self) -> Result<Vec<TaskRecord>,BridgeError>` *(returns full `TaskRecord`s filtered to `Working` — no separate `WorkingTask` struct; the record already carries input/spec/attempts, per Claude review)*.
  - Implement all on `MemoryTaskStore` (a second `Mutex<HashMap<(String,String), (String,bool,i64)>>` for checkpoints; `claim_resume_attempt`: lock the task row, if `resume_attempts >= cap` → `Exhausted`, else `resume_attempts += 1; updated_ms = now` → `Resumable{attempt: resume_attempts}`; `working_tasks`: clone the rows with status `Working`).
- [ ] **Step 4: Implement compile-stubs (bridge-store).** In `sqlite.rs`, add the four new methods to the `impl TaskStore for SqliteStore` block, each body `Err(BridgeError::StoreFailure) // TODO(Task 5): real impl`, EXCEPT keep `create`/`get`/etc. as-is. This makes the trait object compile. (Task 5 replaces these four bodies.)
- [ ] **Step 5: Build the workspace** to surface every `TaskRecord{...}` literal that now needs the 3 fields: `cargo build --workspace 2>&1 | head -40`. Fix each literal (detached arm in `server.rs:1378`; the W3a tests in `workflow_producer.rs` at 301, 794, 849, 902, 952, 1292, 1347, 1395, 1438; sqlite tests at `sqlite.rs:381,400`; the `task_store.rs:169` literal). This additive-field ripple lands in THIS commit.
- [ ] **Step 6: Run** `cargo test -p bridge-core` (new + existing) → green; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo check --workspace`.
- [ ] **Step 7: Commit** `git commit -m "feat(core): TaskStore node-checkpoint + claim_resume_attempt + working_tasks; TaskRecord input/snapshot/attempts (sqlite stubs)"`

---

## Phase E — `SqliteStore` migration + impl

### Task 5: `task_node_checkpoints` table, additive `tasks` columns, `PRAGMA foreign_keys=ON`, real impls

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
    async fn migration_on_old_schema_db_with_cascade_and_fk() {
        // open a file DB with only the OLD tasks table, insert a row, reopen TWICE with new code →
        // columns added (idempotent), row intact, foreign_keys ON, ON DELETE CASCADE works.
        let dir = std::env::temp_dir().join(format!("a2a-w3b-mig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.db");
        {
            use rusqlite::Connection;
            let c = Connection::open(&path).unwrap();
            c.execute_batch("CREATE TABLE tasks(id TEXT PRIMARY KEY, workflow TEXT NOT NULL, status TEXT NOT NULL, result TEXT, error TEXT, created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL);").unwrap();
            c.execute("INSERT INTO tasks(id,workflow,status,created_ms,updated_ms) VALUES('old','wf','working',1,1)", []).unwrap();
        }
        // First reopen: migrates.
        {
            let s = SqliteStore::open(&path).unwrap();
            let got = s.get(&TaskId::parse("old").unwrap()).await.unwrap().unwrap();
            assert_eq!(got.status, TaskRecordStatus::Working);
            assert_eq!(got.input, ""); // default for migrated row
            // checkpoint + cascade
            let old = TaskId::parse("old").unwrap();
            s.put_node_checkpoint(&old, &NodeId::parse("n").unwrap(), "o", true, 2).await.unwrap();
            assert_eq!(s.node_checkpoints(&old).await.unwrap().len(), 1);
        }
        // Second reopen: migration is idempotent (no duplicate-column error), foreign_keys ON.
        {
            let s = SqliteStore::open(&path).unwrap();
            assert_eq!(s.foreign_keys_on().unwrap(), true); // test helper: PRAGMA foreign_keys
            // delete the parent task → checkpoint cascades away
            s.set_terminal(&TaskId::parse("old").unwrap(), TaskRecordStatus::Completed, None, None, 3).await.unwrap();
            s.delete_for_test(&TaskId::parse("old").unwrap()).unwrap(); // test helper: DELETE FROM tasks
            assert_eq!(s.node_checkpoints(&TaskId::parse("old").unwrap()).await.unwrap().len(), 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
```
  *(Add the two small `#[cfg(test)]` helpers `foreign_keys_on()` → `PRAGMA foreign_keys` as bool and `delete_for_test()` → `DELETE FROM tasks WHERE id=?`. Cascade proves the FK is enforced — Codex review.)*
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** In `sqlite.rs`:
  - In `open`/`open_in_memory`, immediately after opening the connection, `conn.execute_batch("PRAGMA foreign_keys = ON;")` (BEFORE `create_schema`, per-connection).
  - `create_schema`: append to the `execute_batch` the `task_node_checkpoints` table (per spec §3D: `PK(task_id, node_id)`, `FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE`). Then run an **idempotent migration** `fn migrate_tasks_columns(conn) -> rusqlite::Result<()>`: read `PRAGMA table_info(tasks)` column names into a `HashSet<String>`, then for each of `input`/`workflow_spec_json`/`resume_attempts`/`last_resume_ms` NOT present, `ALTER TABLE tasks ADD COLUMN …` (with defaults: `input TEXT NOT NULL DEFAULT ''`, `workflow_spec_json TEXT`, `resume_attempts INTEGER NOT NULL DEFAULT 0`, `last_resume_ms INTEGER`). (CREATE TABLE IF NOT EXISTS won't add columns to an existing table — the ALTER is required for old DBs.)
  - Update `create` to INSERT the new columns; update `row_to_task`'s SELECT + mapping to read them (`input` default `''`, `workflow_spec_json` nullable, `resume_attempts`).
  - Replace the Task-4 stubs with real impls: `put_node_checkpoint` (**plain `INSERT INTO task_node_checkpoints(task_id,node_id,output,ok,ts) VALUES(?,?,?,?,?)`** — NOT `INSERT OR REPLACE`; checkpoints are write-once per `(task,node)`, so a duplicate is a logic violation we want surfaced, not masked — Codex review); `node_checkpoints` (`SELECT node_id, output, ok FROM task_node_checkpoints WHERE task_id=?`); `claim_resume_attempt` (a transaction: `SELECT resume_attempts`; if `>= cap` → `Exhausted`; else `UPDATE tasks SET resume_attempts=resume_attempts+1, last_resume_ms=? WHERE id=?` → `Resumable{new}`); `working_tasks` (`SELECT <all task cols> FROM tasks WHERE status='working'` → `Vec<TaskRecord>` via `row_to_task`).
- [ ] **Step 4: Run** `cargo test -p bridge-store` + `cargo clippy --workspace --all-targets -- -D warnings` → green.
- [ ] **Step 5: Commit** `git commit -m "feat(store): task_node_checkpoints + tasks migration (input/snapshot/attempts) + foreign_keys ON; real impls"`

---

## Phase F — executor `run_from(seed)`

### Task 6: `run_from` seed entry + seed validation

**Files:** `crates/bridge-workflow/src/executor.rs`.

- [ ] **Step 1: Failing tests.** In the executor tests: a 3-node graph (a,b → c); call a new `run_from` seeded with `{a:(…,true), b:(…,true)}` and assert only `c` runs (the fakes record which nodes were prompted); a seed with an unknown node id → the stream yields `Err(BridgeError::ConfigInvalid{..})`; a seed missing a non-root node's input (closure violation) → error.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** Refactor `run()` into `run_from(graph, input, run_id, cancel, seed: HashMap<NodeId,(String,bool)>)`; `run()` calls `run_from(graph, input, run_id, cancel, HashMap::new())`. At the top of the stream body, BEFORE the loop: validate the seed — each seeded `NodeId` must exist in `graph.nodes` (else `yield Err(BridgeError::ConfigInvalid{reason:"resume seed: unknown node"})`; return); each seeded non-root node's `inputs` must ALL be seeded (else `yield Err(...closure...)`; return). Then initialize `done` = seeded ids and `outputs` = seed `(text,ok)`; the completion-driven `schedule_ready!` naturally skips seeded nodes (they're in `done`). Keep `run_id` as passed (boot supplies `"{task}-resume-{n}"`).
- [ ] **Step 4: Run** `cargo test -p bridge-workflow` (new + W1/existing) + clippy → green.
- [ ] **Step 5: Commit** `git commit -m "feat(workflow): run_from(seed) resume entry + seed validation (run = empty seed)"`

---

## Phase G — capture in the runner + persist input/snapshot at submit

### Task 7: `TaskStoreSink` persists a checkpoint on each `NodeFinished`

**Files:** `crates/bridge-a2a-inbound/src/workflow_sink.rs` (+ how the runner wires the sink), tests in workflow_producer.rs.

- [ ] **Step 1: Failing test.** In workflow_producer.rs: drive the detached runner over a multi-node workflow with a real `MemoryTaskStore`; after the run, assert `store.node_checkpoints(task)` has a row per node with the right outputs. Also: a store whose `put_node_checkpoint` errors → the task ends `Failed` (the Phase B abort path).
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** Add `output` to the sink's `node_finished` so the checkpoint can be written: **change `WorkflowSink::node_finished` to `node_finished(&mut self, node: &str, ok: bool, output: &str) -> Result<(),BridgeError>`** and `drain_workflow` to pass the event's `output` (it's now in scope from Task 1a). Update `SseSink::node_finished` (ignores `output`). Give `TaskStoreSink` the store + task id: construct `TaskStoreSink::new(store: Arc<dyn TaskStore>, task: TaskId)`; its `node_finished` calls `self.store.put_node_checkpoint(&self.task, &NodeId::parse(node)?, output, ok, now_ms()).await?`. Wire the detached runner to build `TaskStoreSink::new(srv.task_store.clone(), task.clone())` (it still captures the terminal mapping via `terminal`).
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

### Task 9: `spawn_detached_workflow` — final shape (injected graph + run_id + seed)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

> **API-shape settle (both reviewers):** rev1 evolved this signature across Tasks 9 AND 10 (text_parts→input; method→free-fn; seed bolted on later). This task introduces the FINAL shape ONCE so there is no churn in Task 10.

- [ ] **Step 1:** Change `spawn_detached_workflow`'s signature from `(srv, task, text_parts, wf_id, token)` to the FINAL **`spawn_detached_workflow(srv: &Arc<InboundServer>, task: TaskId, input: String, graph: Arc<WorkflowGraph>, run_id: String, token: CancellationToken, seed: HashMap<NodeId,(String,bool)>) -> JoinHandle<()>`**. It no longer resolves the graph from `wf_id` (the caller injects it), takes a pre-joined `input: String`, uses the given `run_id`, and passes `seed` straight to `run_from(graph, input, run_id, cancel, seed)`. Keep the finalizer + token + terminal-write logic. Update the unary detached arm: compute `let input = <join text_parts>;` and call `spawn_detached_workflow(&srv, task, input, srv.workflows.get(wf_id).cloned().expect(...), task.as_str().to_string(), token, HashMap::new())`. Update `spawn_detached_workflow_for_test`/`_with_token_for_test` to resolve the graph from the test server's `workflows` map and pass `run_id = task`, `seed = HashMap::new()` (so Task-7/8 tests still pass).
- [ ] **Step 2:** `cargo test -p bridge-a2a-inbound` + `cargo check --workspace` → green (refactor, no behavior change). Commit `git commit -m "refactor(inbound): spawn_detached_workflow final shape — injected graph + run_id + seed"`

### Task 10a: the boot resume routine (`resume_working_tasks` branch logic)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`, tests in workflow_producer.rs.

- [ ] **Step 1: Failing tests.** In workflow_producer.rs, seed a `MemoryTaskStore` with a `Working` task (input + a `workflow_spec_json` snapshot of `review_graph`) + a checkpoint for `codex` only; build the server `.with_task_store` + `.with_workflows`; call `resume_working_tasks(&srv, cap).await` (free fn — see Step 2). **Determinism (Claude review): the runner is detached and returns no handle — poll `tasks/get` to a terminal status before asserting** (model on workflow_producer.rs:1104), then assert `codex` was NOT re-prompted (the fake backend's prompted-node log lacks `codex`), `claude`+`synth` ran, and `tasks/get` → Completed. Add cases:
  - `workflow_spec_json = None` → Interrupted ("not resumable").
  - parse error (`workflow_spec_json = Some("not json")`) → Interrupted ("unreadable workflow snapshot").
  - **unknown version (`{"v":2,"graph":…}`) → Interrupted** (the real forward-compat door — Claude review).
  - `resume_attempts` already at cap → Interrupted ("resume attempt cap exceeded").
  - terminal node already has a checkpoint → finalized directly (Completed/Failed) WITHOUT prompting and WITHOUT consuming an attempt.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement** `pub async fn resume_working_tasks(srv: &Arc<InboundServer>, cap: u32)` (a free fn mirroring `spawn_detached_workflow`'s `&Arc<InboundServer>` pattern — it must clone the `Arc` into each spawned runner; spec §3E says "method" but a free fn over `&Arc<Self>` is the correct/necessary form, still in `bridge-a2a-inbound` so hexagonal placement holds). For each `srv.task_store.working_tasks()?`:
  - `workflow_spec_json` None → `set_terminal(Interrupted, "not resumable")`; continue.
  - parse the envelope; require `v == 1` — any other version OR a parse error → `set_terminal(Interrupted, "unreadable workflow snapshot")`; continue. Deserialize `graph: WorkflowGraph` from the `"graph"` field.
  - load `node_checkpoints`; find the terminal node id from the graph; if a checkpoint exists for it → finalize directly: `ok==true` → `set_terminal(Completed, output)`; `ok==false` → `set_terminal(Failed, …, output)`; continue (no attempt consumed — narrows the ADR-0010 §8 write-failure gap).
  - else `claim_resume_attempt(&wt.id, cap, now_ms())`: `Exhausted` → `set_terminal(Interrupted, "resume attempt cap exceeded")`; `Resumable{attempt}` → build `seed: HashMap<NodeId,(String,bool)>` from `node_checkpoints`; register a fresh `CancellationToken` in `workflow_cancels` BEFORE spawning; `spawn_detached_workflow(srv, wt.id.clone(), wt.input.clone(), Arc::new(graph), format!("{}-resume-{}", wt.id.as_str(), attempt), token, seed)`.
- [ ] **Step 4: Run** the boot tests + `cargo test -p bridge-a2a-inbound` + clippy → green.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): resume_working_tasks boot routine — short-circuit/seed-resume/Interrupted cases"`

### Task 10b: serve wiring + `resume_attempt_cap` config

**Files:** `bin/a2a-bridge/src/{config.rs,main.rs}`, tests.

- [ ] **Step 1: Failing test.** A config test that `resume_attempt_cap` parses from `[store]` and defaults to 3 when absent.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** `config.rs`: add `pub resume_attempt_cap: Option<u32>` to `StoreConfig`. `main.rs` serve: after building the file-backed task store, instead of `sweep_interrupted`, build the `InboundServer`, then `crate::...::resume_working_tasks(&server, cfg.store.resume_attempt_cap.unwrap_or(3)).await` BEFORE binding the listener. Boot order: open store → build InboundServer → resume → bind → serve. (For the in-memory/no-path branch there's nothing durable to resume — keep or drop `sweep_interrupted` there as a no-op.)
- [ ] **Step 4: Run** `cargo test --workspace` + clippy → green.
- [ ] **Step 5: Commit** `git commit -m "feat(bin): serve runs resume_working_tasks on boot + resume_attempt_cap config (replaces sweep for workflow tasks)"`

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
- [ ] Write `docs/adr/0011-crash-resume.md`: the decision (auto crash-resume from node checkpoints); the components (completion-driven scheduling; additive NodeFinished{output}+fallible sink; run_from seed; lean task_node_checkpoints + versioned snapshot; boot resume on InboundServer + cap; terminal-checkpoint short-circuit narrowing the W3a write-failure gap); the dual-design+dual-review provenance + the adopt-completion-driven decision + the cancel-drain blocker the plan review caught; the live-gate result; follow-ons (streaming reattach, retry policy, history query API, retention/prune). Commit with the controller trailer.

---

## DoD §9 → tasks
| DoD | Task |
|-----|------|
| 1 (completion-driven; streaming green; completion-order + write-ahead + cancel-drain tests) | 1a (event), 1b (scheduler + tests) |
| 2 (NodeFinished{output}; sink Result; drain aborts) | 1a (event), 2 (sink Result), 7 (node_finished output) |
| 3 (run_from seed; unknown/closure rejection) | 6 |
| 4 (WorkflowGraph serde; unparseable + unknown-version handled) | 3 (serde), 10a (unparseable/v≠1→Interrupted) |
| 5 (checkpoints table + migration + foreign_keys + cascade) | 5 |
| 6 (TaskStore methods + create persists; both impls) | 4 (core+stubs), 5 (sqlite) |
| 7 (runner checkpoints; write-failure fails task) | 7 |
| 8 (boot resume; short-circuit; Interrupted cases; replaces sweep; token before spawn) | 10a, 10b |
| 9 (resume_attempt_cap config; poison cap) | 10b (config), 11 |
| 10 (gated live) | 13 |
| 11 (fmt/clippy/coverage) | 12 |

## Notes for the implementer
- **Riskiest = Task 1b** (completion-driven scheduler rewrite). Task 1a lands the `NodeFinished{output}` field over `join_all` first (pure ripple) so 1b is the scheduler change ALONE. The W1 streaming test is the oracle; the cancel-drain test (1b Step 3) and the write-ahead test (1b Step 5) are the new guards. Codex confirmed the `FuturesUnordered` + `&this` capture borrow-checks (`AgentRegistry: Send+Sync`).
- **Cancel = drain, NOT break.** On `cancel`, set `stop_scheduling` and keep draining `inflight` so every in-flight node runs its `backend.cancel()`+`forget_session()`. A `break` strands ACP sessions — this was the top dual-review blocker.
- **`run cargo check --workspace` after every task** — it catches consumers the per-crate test commands miss (the `bin/a2a-bridge/src/main.rs` `NodeFinished` arm was a rev1 miss).
- **Additive `TaskRecord` fields (Task 4)** ripple to every `TaskRecord{...}` literal — the build surfaces them (the known sites are enumerated in Task 4 Step 5). Task 4 also adds `SqliteStore` compile-stubs (returning `StoreFailure`) so the workspace stays green before Task 5 fills them.
- **`working_tasks()` returns `Vec<TaskRecord>`** (no separate `WorkingTask` struct — the record carries input/spec/attempts).
- **Checkpoints are write-once** per `(task,node)` → plain `INSERT` (not `INSERT OR REPLACE`); a duplicate surfaces a barrier/resume bug rather than masking it.
- **Sink `node_finished` gains `output`** (Task 7) — a second small signature change after Task 2's `Result` change; sequence as written (1a event → 2 Result → 7 output param).
- **`spawn_detached_workflow` final shape** lands once in Task 9 (`input: String`, injected `graph`, `run_id`, `seed`); `resume_working_tasks` is a free fn `(srv: &Arc<InboundServer>, cap: u32)` from the start (Task 10a) — no later churn.
- Firewall: `~/code/a2a-local-bridge` black-box only. `serve` reads a fixed `a2a-bridge.toml` from CWD. workflow_producer tests run under `-p bridge-a2a-inbound`. Controller docs (this plan, ADR-0011) carry the `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer; task commits do NOT.
