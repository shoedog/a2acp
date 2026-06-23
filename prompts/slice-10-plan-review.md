You are doing a rigorous, adversarial PLAN REVIEW (read-only) of the implementation plan for "Slice 10 — B2:
Weighted Fan-out Panel" for the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator).
READ-ONLY: read the plan + the binding spec + the real code; do NOT edit/build/test.

- PLAN: `docs/superpowers/plans/2026-06-22-slice-10-fanout-panel.md` (8 TDD tasks T1–T8).
- BINDING SPEC: `docs/superpowers/specs/2026-06-22-slice-10-fanout-panel.md` — the `## v2` section (SF-FIX-1..6).
- The plan claims specific `file:line` anchors and exact type/signature changes. VERIFY each against the real code.

Key code to verify the plan against:
- `crates/bridge-workflow/src/executor.rs` — `run_node` (~:102), the two `Update::Usage` ignore sites (~:169/:275),
  `WorkflowEvent::NodeFinished` (~:80), `NodeFut` (~:61), the `outputs` map + `schedule_ready!` fan-in injection
  (~:455/:484-497), `run_from*` seed type `HashMap<String,(String,bool)>` (~:373).
- `crates/bridge-workflow/src/graph.rs` — `WorkflowGraph`/`WorkflowNode` (the plan adds `panel: Option<PanelConfig>`).
- `crates/bridge-core/src/orch.rs` — `OrchEventKind::NodeFinished` (~:66), `UsageSnapshot` (~:37).
- `crates/bridge-core/src/task_store.rs` — `TaskStore` trait `put_node_checkpoint_sequenced` (~:166) +
  `node_checkpoints` (~:130), `fold_journal_to_snapshot` (~:270), `TaskProgressSnapshot.checkpoints` (~:246),
  InMemory store (~:317/:433/:456/:535/:760).
- `crates/bridge-store/src/sqlite.rs` — migration `migrate_tasks_columns` (~:150), `put_node_checkpoint_sequenced`
  (~:634), `node_checkpoints` (~:506).
- `crates/bridge-coordinator/src/detached.rs` — `WorkflowSink` trait + `node_finished` (~:182/:186),
  `drain_workflow` (~:207), `DetachedProgressSink::node_finished` (~:301), `FrameKind::NodeFinished` (~:69),
  `frame_from_orch` (~:96), `encode_workflow_spec`/`WorkflowSpecEnvelope` (~:1310/:1322), `resume_working_tasks`
  seed build (~:1423), `spawn_detached_workflow` seed (~:1115).
- `crates/bridge-a2a-inbound/src/server.rs` — `SseSink::node_finished` (~:1850), snapshot-replay NodeFinished
  frame (~:1200).
- `bin/a2a-bridge/src/config.rs` — `WorkflowToml`/`WorkflowNodeToml` (~:156), `load_workflows` (~:840).
- `bin/a2a-bridge/src/main.rs` — `run-workflow` printer match (~:2719).

{{input}}

GROUND every finding in real `file:line`. Pressure-test the PLAN specifically:

1. **Compile-green per task (the load-bearing claim).** The plan asserts each task leaves the tree compiling +
   tests passing. Trace the type ripples: T1 adds a field to `OrchEventKind::NodeFinished` — does it enumerate
   EVERY destructure/construction site (fold, sqlite event build, detached `:991`, all tests)? T2 changes
   `node_checkpoints` from a 3-tuple to a 4-tuple and `put_node_checkpoint_sequenced` arity — are ALL impls AND
   call sites (incl. `resume_working_tasks`, `workflow_producer.rs` fakes) handled in the SAME task, or does the
   tree break between tasks? T3 adds the `WorkflowSink::node_finished` 4th param — every impl + the re-export?
   T4 changes `run_node`'s return + the `outputs` map + `NodeFut` — every early-return and the `schedule_ready!`
   closure? T5's 25 `WorkflowGraph {` literals — is the count real and the file list complete? T6's seed-type
   change — every `run_from*` caller? Flag any task that would NOT compile as written.

2. **Ordering correctness.** Is the bottom-up order (T1 event → T2 store → T3 wire → T4 capture → T5 weights →
   T6 resume → T7 workflow → T8 gate) actually dependency-correct? Does T3 depend on T2's new
   `put_node_checkpoint_sequenced` signature (the plan says `DetachedProgressSink` passes usage to it in T3)?
   Does T4's `{{workflow.costs}}` depend on the `outputs` map already carrying usage? Any forward reference?

3. **SF-FIX-4 durability (the BLOCKER).** Trace the full carry chain the plan specifies: capture (T4) → journal
   event (T1) + checkpoint column (T2) → `node_checkpoints` read (T2) → resume seed (T6) → `run_from` → fan-in
   `{{workflow.costs}}`. Is it ACTUALLY closed? The plan keeps `TaskProgressSnapshot.checkpoints` a 4-tuple and
   defers snapshot-replay surfacing — does the resume seed read `node_checkpoints` (table) DIRECTLY (not the
   snapshot)? Confirm at `detached.rs:1423`. Will an OLD pre-B2 checkpoint row (NULL `usage_json`) fold/seed as
   `None` without panicking? Is the `ALTER TABLE ADD COLUMN usage_json` migration idempotent + safe on old DBs?

4. **The weights-on-graph decision.** The plan puts `panel: Option<PanelConfig>` on `WorkflowGraph` with
   `#[serde(default, skip_serializing_if)]` so weights ride the durable spec snapshot. Is that ADDITIVE-SAFE for
   resume of an in-flight pre-B2 task (old snapshot JSON has no `panel` key)? Is `encode_workflow_spec`
   (`json!({"v":..,"graph":graph})`) actually going to serialize the new field, and does `WorkflowSpecEnvelope`
   deserialize it? Is the alternative (`WorkflowRunContext`) materially better/worse? Confirm or refute.

5. **`{{workflow.costs}}`/`{{workflow.weights}}` seam.** Verify the `NodeId` charset truly bans `.` (so the
   reserved namespace can't collide), and the `template.rs` renderer matches dotted tokens. Are these vars
   injected only where they should be (costs for fan-in nodes only; weights from `graph.panel`)? Back-compat:
   do existing workflows that don't reference these vars render byte-identically?

6. **Test quality.** Are the TDD tests real failing-tests-first (not asserting trivia)? Does T4's smoke test
   actually prove per-node capture (used>0 from the EVENT, not markdown)? Does T6 prove usage survives resume?
   Does the live-gate (post-tasks) assert RAW usage fields, not the synth markdown (the truncation trap)? Any
   test that would pass even if the feature were broken?

7. **Missing pieces / wrong anchors / scope.** Any wrong `file:line`. Any SF-FIX the plan fails to realize. Any
   step with a placeholder, an undefined type/fn, or a signature mismatch between tasks. Anything the plan must
   add or cut. Is the per-task granularity right (any task too big to be one safe commit)?

OUTPUT: a numbered findings list, each tagged `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete
fix. End with `PLAN VERDICT: ready-to-implement | needs-revision`.
