You are doing a rigorous, adversarial PLAN REVIEW (read-only) of the implementation plan for "E3 — Parallel Batch
Dispatch" for the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). READ-ONLY: read the plan +
the binding spec + the real code with read-only tools; do NOT edit/build/test. Be terse; end with a bounded STOP.

- PLAN: `docs/superpowers/plans/2026-06-26-e3-batch.md` (8 TDD tasks T1–T8).
- BINDING SPEC: `docs/superpowers/specs/2026-06-26-e3-batch.md` — the `## v3` (RR-FIX-1..14, D11–D14) + `## v2`
  (SR-FIX-1..17) sections supersede the v1 body.
- The plan claims specific `file:line` anchors + exact type/signature changes. VERIFY each against the real code.

E3 = `run-batch <workflow> --manifest <file>` submits N independent workflow runs to a running `serve`, run
concurrently under a serve-wide `Semaphore` cap, durable. Each child reuses `spawn_detached_workflow`; a durable
`batch` parent record (status / cancel / crash-safe tail-resume); a `claim_batch_child` store primitive owns
spawn-ownership. The admission loop + resume + roll-up are free fns in `bridge-coordinator::batch` over a `BatchDeps`
bundle, called from BOTH the `InboundServer` (serve) and `Coordinator` (mcp) sides.

Key code to verify the plan against:
- `crates/bridge-core/src/task_store.rs` — `TaskRecord` (~:60), `TaskRecordStatus` (~:12, variants
  `Working|Completed|Failed|Canceled|Interrupted`), the `TaskStore` trait (~:92), the `journal_fold_inputs`
  default-body precedent (~:119/:201 `StoreFailure`). T1 adds `batch_id`/`item_id`; T3 adds 8 default-bodied methods.
- `crates/bridge-core/src/ids.rs:5` — `id_newtype!` (T1 adds `BatchId`).
- `crates/bridge-store/src/sqlite.rs` — the `Mutex<Connection>` store + `migrate_tasks_columns` (PRAGMA-guarded
  `ALTER`), `cancel_if_working` CAS (`WHERE status='working'`, ~:482/:486). T4 adds the migration + transactional
  `claim_batch_child`.
- `crates/bridge-coordinator/src/detached.rs` — `DetachedDeps` (~:257: `task_store, executor, workflows,
  workflow_cancels, progress_hubs, clock` — NO `allowed_cwd_root`), `spawn_detached_workflow(deps, task, input,
  graph, run_id, token, seed, ctx, hub) -> JoinHandle<()>` (~:1186), `finalize_detached(store, progress_hubs, task,
  status, result, error, hub)` (~:1143), `new_detached_task_id` (~:1120), `resume_working_tasks` (coordinator-side,
  ~:1423).
- `crates/bridge-a2a-inbound/src/server.rs` — `InboundServer` (~:75, fields incl. `allowed_cwd_root: Option<String>`
  ~:146), its OWN `resume_working_tasks(srv, cap)` (~:2155, the SERVE boot path), `detached_deps()` (~:2049), the
  `match method` dispatch (~:713) + `Session*` arms (~:720-726), `TaskStatusDto`/`list_tasks` (~:3177).
- `crates/bridge-coordinator/src/coordinator.rs` — `Coordinator` (`allowed_cwd_root: Option<SessionCwd>` ~:108),
  `run_workflow` (~:359), `resume()` (~:499), `detached_deps()` (~:165).
- `crates/bridge-coordinator/src/params.rs:263` — `OpParams::validate_cwd` (a `&self` method; T8 extracts a free
  `validate_cwd(s, root, field_label)`).
- `bin/a2a-bridge/src/main.rs` — serve wiring `InboundServer::new(...).with_*` (~:4421) + the SERVE boot
  `resume_working_tasks(&server, resume_cap)` (~:4466); the `mcp` `Coordinator::new` (~:4118); `run-workflow --serve`
  POST client (~:2544). `config.rs:117` `RegistryConfig` (T5 adds `[batch]`).

{{input}}

GROUND every finding in real `file:line`. Pressure-test the PLAN specifically:

1. **Compile-green per task.** T1's `batch_id`/`item_id` additive field — does the plan update EVERY `TaskRecord { … }`
   literal (it claims 48 across 6 files — verify the count + that ALL are named/covered) so each crate compiles? T3's
   8 trait methods — do default bodies keep ALL `impl TaskStore` (the 2 real + the 3 delegating test-doubles —
   `FailingCheckpointStore` etc.) compiling untouched? Does `BridgeError::StoreFailure` exist (the plan corrected
   `Unsupported`→`StoreFailure` — confirm no `Unsupported` is referenced)? Does T5's `BatchDeps`/`BatchRuntime` shape
   compile (the `DetachedDeps` superset + `SessionCwd` normalization)? Flag any task that would NOT compile or has a
   forward reference.
2. **T4 `claim_batch_child` (the idempotency keystone).** Is the transactional insert-or-select correct + race-safe
   under the `Mutex<Connection>` (no `.await` held across the lock)? Does the partial `UNIQUE(batch_id,item_id) WHERE
   batch_id IS NOT NULL` index compose with the existing `tasks` PK + the non-clobbering `create`? Does the migration
   stay idempotent on re-open (PRAGMA-guarded ALTERs + `IF NOT EXISTS`)? Does the `ChildClaim` mapping
   (Created/ExistingWorking/ExistingTerminal) drive the right spawn/skip decision in T6?
3. **T6 admission loop (the core).** Is the biased `select! { cancel, acquire_owned }` faithful (cancel never creates
   a row)? Is the permit acquired BEFORE claim/spawn and released ON child completion (carried into the JoinHandle)?
   Is the re-check-after-claim → `finalize_detached(Canceled)` for a suppressed spawn correct (RR-FIX-2, no stranded
   Working row)? Is the loop the SINGLE canceller of its in-flight children (RR-FIX-9), and does `cancel_batch` (T8)
   only CAS + fire the batch token (not enumerate children)? Does last-permit/terminal settle via `is_settleable`
   exactly once? Any deadlock (acquire while holding a store lock; awaiting a child that blocks on a permit)?
4. **T7 resume (both boot sites).** Does the plan wire `resume_batches` into BOTH the inbound `resume_working_tasks`
   (server.rs:2155, the SERVE path) AND `Coordinator::resume()` (the mcp path) — NOT only one? Is the non-batch
   partition a call-site filter (`batch_id.is_none()`), leaving `working_tasks()`'s contract intact (RR-FIX-9)? Does
   the orphan sweep (Working child of a terminal/absent batch → Interrupted, RR-FIX-4) + corrupt-plan→Failed
   (RR-FIX-5) + Canceling-resume cover every interleaving with no stranded/double-resumed task? Does resume re-run
   working children THROUGH the semaphore with FRESH tokens (cap-on-boot + Q3)?
5. **Ordering + seam correctness.** Is bottom-up T1→T8 right (no forward refs)? Does T6 depend only on T1–T5, T7 on
   T6, T8 on T6/T7? Does the plan's claim that the serve path uses the INBOUND `resume_working_tasks` (not the
   coordinator's) match the real wiring (main.rs:4466)?
6. **Test quality.** Are the TDD tests real failing-first + non-tautological? Does T6's harness actually exercise the
   cap (max-in-flight assertion), the serve-wide cap across TWO batches sharing one semaphore, cancel-wakes-a-
   blocked-loop, suppressed-finalize, and claim-existing-not-respawned? Does T7 actually prove no-double-spawn +
   cap-on-boot + orphan-sweep? Any test that passes even if the feature is broken?
7. **Faithfulness to spec v3 + missing/over-built.** Each RR-FIX-1..14 + the surviving SR-FIXes + D1–D14 → a task
   (the plan's self-review claims a full map — verify a sample)? Any wrong `file:line`. Any step with a
   placeholder/undefined type. Is per-task granularity right (is T6 or T7 too big to land green in one task)? Anything
   the plan must add or cut?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` (in the plan OR the code)
+ a concrete fix. End with `PLAN VERDICT: ready-to-implement | needs-revision`. Then STOP.
