# Fable review — Coordinator migration spec (roadmap #10)

_Read-only, repo-rooted, deepest-reasoning lens on an M-H migration. Verdict
REVISE. Corroborates + extends the codex lens. All load-bearing claims verified
against source._

## BLOCKER

**B1 — D1's "two stores" splits one field serving two concerns; slice-7 KEEP omits `store`.**
The adapter has ONE `store: Arc<dyn SessionStore>` (server.rs:90) serving BOTH
lifecycle (`store.put(task, session)` before dispatch — server.rs:800/838/2336/2374;
translator `put_pending` — translator.rs:170) AND delegation/fanout/cancel
bookkeeping (`set_peer_task`/`request_cancel`/`is_fanout` — server.rs:1624/2758/1747).
v1's "keep a separate delegation store" split-brains task→session across two stores
(`cancel_task` reads `session_for` from the adapter store, server.rs:2901, while a
coordinator turn writes the coordinator store, coordinator.rs:311). Slice-7's
"7 KEEP" omits `store` — deleting it breaks every delegate/fanout path. Arithmetic:
21 = 9 delegated + 4 shared Arcs + 7 KEEP = 20; `store` is the missing 21st.
**Fix:** identity-share the ONE in-memory store — pass it into `Coordinator::new`
as `session_store`. D1's conclusion (no durability flip) survives; the mechanism
(separate instances) must not.

**B2 — Slice 6 (warm turns → `coordinator.prompt`/`continue_turn`) is not behavior-preserving.**
- Streaming Local arm (`spawn_local_producer`, server.rs:1417-1527) forwards SSE
  incrementally; `Coordinator::prompt` is collect-only (`collect_turn`→one
  `TurnOutput`, coordinator.rs:276-389). Delegating changes the wire from
  incremental SSE to nothing-until-done.
- Unary: `coordinator.prompt` mints its own task-id (`prompt-{now}-{seq}`,
  coordinator.rs:222/307) and hides the session, so the adapter can't
  `store.put(routed.task, dispatch.session)` (server.rs:2374). Consequence:
  `CancelTask` by wire task-id degrades from cancelling the real warm session
  (server.rs:2901) to a synthetic no-op; `get_task`'s WORKING heuristic
  (server.rs:3213) stops seeing warm turns. The unary response also carries
  `"status":[chunks]` (server.rs:2584) that `TurnOutput` can't reconstruct — wire
  loss.
**Fix:** reclassify the entire Local send arm (streaming AND unary) as
adapter-resident over the shared `session_manager` (it's already thin —
`warm_local_dispatch` ~40 lines over `checkout_turn`), or add a streaming,
caller-task-id Coordinator API. As written, slice 6 ships wire changes at the
highest-risk point.

**B3 — BatchRuntime identity missing from the slice-1 shared set.**
`BatchRuntime` is `Clone` over `Arc<Semaphore>` + `Arc<Mutex<HashMap<BatchId,
CancellationToken>>>` (batch.rs:24-30). `main` calls `batch_runtime(&cfg)` for the
server (main.rs:6109); a second call for `Coordinator::new` → two semaphores + two
`batch_cancels`. Between slice 2 (RunBatch→coordinator's semaphore) and slice 4
(boot resume still adapter `batch_deps(&srv)`→adapter semaphore, server.rs:2195),
the serve-wide cap silently doubles and `CancelBatch` can't cancel boot-resumed
items — the E3 invariant breaks mid-migration. **Fix:** one `BatchRuntime`, clone
into both.

## MAJOR

- **M1 — run_workflow rejects overrides the A2A arm silently drops.**
  `Coordinator::run_workflow` errors on `agent/model/effort/mode`
  (coordinator.rs:393); the A2A Workflow route drops `AgentOverride` by design
  (server.rs:416). Slice-4 delegation turns today-succeeding `a2a-bridge.model`
  submits into `InvalidRequest`. Wrapper must strip overrides first.
- **M2 — cancel_task durable-arm delegation is a different state machine.** A2A
  fires token → returns CANCELED immediately, runner writes its own terminal
  (server.rs:2777-2795), flips `cancel_if_working` ONLY when no token, echoes true
  terminal on races (2807-2818). `Coordinator::cancel_task` fires token AND
  immediately `cancel_if_working` (coordinator.rs:506) — external Working→Canceled
  while the runner lives, returns only `bool` (loses the echo). Wrapper must retain
  token-fired-early-return + true-state re-read.
- **M3 — D2 evidence/visibility wrong in two places.** (a) `bindings` is NOT
  dual-written — Coordinator's only touch is the dead `_deferred_cold_bindings`
  placeholder (coordinator.rs:231); the real dual-writers are
  `workflow_cancels`/`progress_hubs` + `workflow_runs`. Registry identity is
  load-bearing (hot-reload reconcile applies to ONE Arc, main.rs:5986). (b)
  `pub(crate)` accessors are impossible cross-crate — must be `pub`.
- **M4 — double-boot-resume hazard.** Slice 4 must REPLACE
  `resume_working_tasks(&server, cap)` (main.rs:6157) with `coordinator.resume()`,
  never both — two scans double-spawn runners (journal/checkpoint corruption).
- **M5 — slice 5 touches the warm/cancel path before slice 6's gate.**
  `session_clear`→`coordinator.clear(force=true)` fires in-flight warm abort tokens
  (both biased selects, server.rs:1465/2400) — but the force-reset live-gate is
  scheduled for slice 6. Move it to slice 5.

## MINOR
- `SessionStatusDto` lacks `windowFraction` + casing differs (server.rs:2957 vs
  coordinator.rs:40); delegating `session_status` buys zero dedup — don't.
- Clock asymmetry: adapter uses `now_ms()`/fresh `SystemClock` (server.rs:2082/2471);
  post-migration routes through the injected clock (test-fidelity only).
- Batch not-configured error text drifts if the wrapper guard is dropped
  (`"batch not configured"` server.rs:3287 vs `InvalidRequest{...}` coordinator.rs:189).
- Golden-wire (15 tests) only trips shapes it encodes; add CancelTask response
  variants + unary-local `status`-chunks BEFORE slices 4/6.
- Cite drift: `run_workflow` at coordinator.rs:392 (spec said :414). All other
  spot-checked cites accurate.
- Goal overstatement: after slice 7, `spawn_local_producer`'s drain loop remains a
  parallel `collect_turn` — "ships once" holds for STATE, not the streaming drain.

## ANSWERS
1. **D1:** direction right, framing wrong — in-memory loss is a narrow latent gap;
   file-backing is WORSE (durable cancel latches on reusable ids kill fresh tasks).
   Keep in-memory; SHARE the instance (B1).
2. **D2:** sharing mandatory; invert wiring — Coordinator owns the maps
   (coordinator.rs:150), build it FIRST, adapter adopts via `pub` accessors; no
   `::new` variant (two construction shapes is the divergence this kills). Add
   store/BatchRuntime/registry to the set.
3. **D3:** confirmed detached-only (coordinator.rs:392); asymmetry acceptable — but
   the honest end-state is A2A's ENTIRE streaming surface stays adapter-resident.
4. **Slice-1 atomicity:** no split-brain BEFORE slice 1 (no Coordinator exists yet);
   real constraint = shared before slice 2 (`run_batch`→`detached_deps`→
   `workflow_cancels`+`progress_hubs`). Under-captured: BatchRuntime, store, registry.
5. **Ordering:** two hidden couplings — clear(force) fires warm aborts pre-gate (M5);
   batch/resume split runs divergent BatchRuntimes unless shared (B3) + slice 4 must
   exclusively replace resume (M4). Routing `status` in slice 3 is safe.
6. **Invariants:** #1/#3 survive as sliced; #5 survives ONLY if slice 6 keeps the
   streaming arm adapter-resident (B2); #2 structurally untouched but the durable-arm
   swap (M2) misreports terminal state on the wire.

## VERDICT
**REVISE** — architecture sound, D1/D3 point right, but the spec as written ships
wire changes: fix D1 to instance-sharing + repair slice-7 KEEP arithmetic (B1); add
BatchRuntime + store + registry to the slice-1 identity set (B3/M3); rewrite slice 6
to keep the Local send arm adapter-resident or scope a streaming Coordinator API
(B2); plus the M1/M2 wrapper caveats and the M5 live-gate move.
