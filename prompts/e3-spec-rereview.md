You are doing a focused, adversarial RE-REVIEW (read-only) of the REVISED spec "E3 — Parallel Batch Dispatch" for the
a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). The first dual spec-review found 2 BLOCKER + 7
MAJOR + several MINOR/NIT; all were folded into the spec's **`## v2` section (BINDING)** as SR-FIX-1..17. YOUR JOB:
verify each fold actually RESOLVES its finding, and hunt for NEW issues the v2 decisions introduce — especially in the
re-homed seam and the resume/cancel concurrency. READ-ONLY: read the spec + the real code with read-only tools; do NOT
edit/build/test. Be terse; end with a bounded STOP.

The spec: `docs/superpowers/specs/2026-06-26-e3-batch.md` — read the **`## v2` section FIRST** (it supersedes the v1
body where they conflict), then the v1 body for context.

E3 = `run-batch <workflow> --manifest <file>` submits N independent workflow runs to a running `serve`, run
concurrently under a serve-wide concurrency cap, durably; each child reuses `spawn_detached_workflow`; a durable
`batch` parent record gives status / cancel / crash-safe tail-resume.

The v2 folds to validate (RESOLVED / PARTIALLY-RESOLVED / NOT-RESOLVED for each):
- **SR-FIX-1 (BLOCKER)** — seam re-homed OFF the mcp-only `Coordinator` (`main.rs:4118`) onto **free functions in
  `bridge-coordinator::batch` over a `BatchDeps` bundle** (mirror `DetachedDeps` @ `detached.rs:257`) + a
  `batch_semaphore: Arc<Semaphore>` **field on `InboundServer`** (serve wiring @ `main.rs:4421`, sized from `[batch]`),
  with `Coordinator::{run_batch,…}` as thin wrappers. Inbound arms call the free fns via `srv` state, like the
  `message/send` detached arm (`server.rs:2495`). VERIFY: is `InboundServer` (`server.rs:75`) actually the right owner
  (does it already hold `workflow_cancels`/`progress_hubs`/`task_store`)? Does "one semaphore per PROCESS" truly give
  the serve-wide cap (serve and mcp are separate processes)? Any state the free fns need that lives only on
  `Coordinator` and is NOT on `InboundServer`?
- **SR-FIX-2 (BLOCKER)** — CAS status: `cancel_batch_if_working` + `settle_batch_if_status(expect,new)` (`WHERE
  status=?`, mirroring `cancel_if_working` @ sqlite.rs:476). VERIFY the loop settles `Completed` only from `working`,
  and the cancel path's two-step `working→canceling→canceled` is race-free against the loop.
- **SR-FIX-3 (MAJOR)** — boot scans `active_batches() = Working | Canceling`; `Canceling` → cancel working children,
  admit no tail, settle `Canceled`. VERIFY no stranded-batch path remains (crash at each transition).
- **SR-FIX-4 (MAJOR)** — partial `UNIQUE(batch_id,item_id) WHERE batch_id IS NOT NULL` + idempotent child-create.
  VERIFY this actually closes the concurrent admit-vs-resume double-spawn (not just the single-actor case).
- **SR-FIX-5 (MAJOR, resolves Q1)** — lazy CAS-settle in `batch_status` when `pending=0 && running=0`. VERIFY it
  cannot settle while a child is still mid-flight, and is idempotent vs the loop + boot settle.
- **SR-FIX-6 (MAJOR)** — roll-up buckets `ok|failed|canceled|running|pending`, terminal = ok+failed+canceled.
  VERIFY the settle predicate (`pending=0 && running=0`) and the terminal accounting agree (no child status falls
  through the buckets — walk all of `TaskRecordStatus`).
- **SR-FIX-7/8/9/10** — `BatchSummary` pure-fn in `batch.rs`; biased select! admit + re-check-before-spawn;
  don't redefine `working_tasks()`; single `resume_all` routing by `batch_id`. VERIFY SR-FIX-10's `resume_all`
  partition resumes EVERY working task exactly once by exactly one owner (no task both batch-resumed AND
  plain-resumed; none dropped), and that it is called from BOTH boot sites (serve `main.rs:4466` + mcp
  `Coordinator::resume`).
- **SR-FIX-11..17** — items_json `{"v":1,…}` envelope; default-bodied trait methods; server-side cwd validation;
  `batch_status -> BatchSummary` + `Canceled/Canceling` spelling; "not verbatim reuse"; pool-not-bounded labeling;
  free `validate_cwd` extract. VERIFY each is internally consistent with the rest of v2.

{{input}}

GROUND every finding in a real `file:line`. Pressure-test the V2 DECISIONS specifically:
1. **Does the re-homed seam (SR-FIX-1) actually work?** Trace what `spawn_detached_workflow` needs (`detached.rs`) and
   confirm `BatchDeps` can supply it from `InboundServer` state without a `Coordinator`. Does the per-process
   semaphore + the inbound arms compose, or is there still a Coordinator dependency hiding (e.g. `self.workflows`
   lookup, `allowed_cwd_root`, the clock)? Is there a DEADLOCK risk in the free-fn admission loop holding a store lock
   across `acquire_owned().await`?
2. **CAS + lazy-settle + cancel interplay (SR-FIX-2/3/5/6 — the riskiest new surface).** Walk concurrent: the
   admission loop about to `settle_batch_if_status(working→completed)` AT THE SAME TIME as `cancel_batch`
   (`working→canceling`). Does CAS guarantee exactly one wins with no lost update? After a `canceling` win, does the
   loop correctly STOP admitting + not re-settle to `completed`? Can lazy-settle in `batch_status` settle a
   `canceling` batch to `completed` (it must not)? Any interleaving where a batch ends in two terminal states or none?
3. **`resume_all` (SR-FIX-10) exactly-once.** Can a child be resumed twice (once as a batch child, once as a plain
   task) if the partition predicate (`batch_id.is_some()`) disagrees with `active_batches()` membership (e.g. a
   working child whose batch is already `Completed`/absent)? Can a working child be DROPPED (its batch not in
   `active_batches`, but it's filtered out of the plain path)? Is the cap honored across BOTH partitions on boot?
4. **Idempotency (SR-FIX-4) under the real migration.** Does a partial unique index compose with the existing
   `tasks` PK + the non-clobbering `create` (`task_store.rs:93`)? On a UNIQUE violation during admit, does the spec's
   "treat conflict as existing child" actually recover (re-spawn the existing row) or could it skip a never-spawned
   item? Is the migration still idempotent (re-open) with the new index?
5. **New issues from v2.** Does `Canceling` (a 5th batch state) need handling anywhere the spec missed (list output,
   `BatchSummary`, the CLI roll-up render)? Does server-side cwd validation (SR-FIX-13) leave any client/serve
   responsibility gap? Does the default-bodied trait method (SR-FIX-12) risk a real store silently no-op'ing a batch
   op (e.g. `MemoryTaskStore` used in a real path)? Does the `validate_cwd` extract (SR-FIX-17) change `OpParams`
   behavior?
6. **Still-open / under-specified.** Any SR-FIX that is PARTIALLY or NOT resolved. Any decision (D1–D10) or Q (Q1–Q7)
   still ambiguous enough to block planning. Any NEW `file:line` correction. Is the slice still right-sized after the
   folds, or did v2 grow it past one plan?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. For each
SR-FIX-1..17 state RESOLVED / PARTIALLY-RESOLVED / NOT-RESOLVED (one line each). End with
`RE-REVIEW VERDICT: ready-to-plan | needs-revision | needs-spike`. Then STOP.
