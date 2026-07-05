# VERIFICATION — Coordinator migration (#10)

Branch: `feat/coordinator-migration`. Spec:
`docs/superpowers/specs/2026-07-05-coordinator-migration.md` (v2, dual-reviewed).

**This is a BEHAVIOR-PRESERVING migration** — `InboundServer` becomes a thin
adapter over ONE `Arc<Coordinator>`, deleting its parallel lifecycle-state copies.
No A2A wire change (golden-wire + a live gate must stay green). The correctness
gate is the inverse-and-stronger one: **the entire pre-existing suite passes
UNCHANGED after every slice**, plus new tests that PIN the shapes golden-wire does
not encode (the slice-4/6 drift risks). This ledger updates at each slice.

Baseline on `main` (post-#9): **1424 passed / 0 failed / 12 ignored**.

## Commands run + results

### Harness gap test (task #17) — pre-slice
```
cargo test -p bridge-a2a-inbound --test workflow_producer warm_unary_cancel_by_wire_id_hits_real_session -j 1
```
Added `warm_unary_cancel_by_wire_id_hits_real_session` to `workflow_producer.rs`:
a warm UNARY (Local, non-workflow) send held open by a gated backend, cancelled by
its wire task-id mid-turn. Asserts `CancelTask` fires `backend.cancel` on the SAME
session the turn runs under (`ctx-{ctx}-g{gen}`), with an explicit discrimination
guard that this differs from the synthetic `session-{task}` store-miss fallback.
This pins the behavior slice 6 would break if it delegated warm turns to
`Coordinator::prompt` (synthetic id, hidden session). Golden-wire does NOT encode
this shape. **Green.** Full `workflow_producer` file: 48/0/0 (was 47).

### Slice 1 — one Coordinator; adapter adopts the shared-identity set
```
cargo test --workspace -j 1     → 1426 passed / 0 failed / 12 ignored
cargo clippy -p bridge-coordinator -p bridge-a2a-inbound -p a2a-bridge -j 1  → clean
cargo test -p bridge-a2a-inbound --test golden_wire -j 1   → 15/0/0
```
1426 = baseline 1424 + the two new tests (harness gap + shared-Arc identity).
Behavior-preserving confirmed: no existing test changed.

**Changes:**
- `bridge-coordinator/src/coordinator.rs`: added `pub` shared-state accessors (D2 —
  cross-crate, so `pub` not `pub(crate)`): `task_store` / `session_store` /
  `registry` / `policy` / `executor` / `workflows` / `bindings` / `workflow_cancels`
  / `workflow_runs` / `progress_hubs` / `permission_registry` / `batch` /
  `allowed_cwd_root`. Each returns a clone of the owned Arc (identity preserved).
- `bridge-a2a-inbound/src/server.rs`: `InboundServer` gains
  `coordinator: Option<Arc<Coordinator>>` + `with_coordinator(coord)` builder that
  re-points every shared field to the Coordinator's instance, + a `coordinator()`
  accessor. New in-crate test `with_coordinator_shares_state_identity` asserts
  `Arc::ptr_eq` across all 13 shared fields (the anti-split-brain guarantee).
- `bin/a2a-bridge/src/main.rs` (serve path): build ONE `Coordinator` FIRST (D2)
  with the ONE in-memory `store` instance-shared as `session_store` (D1) and ONE
  `BatchRuntime` (B3); the SessionManager switches to `new_with_clock(SystemClock)`
  sharing that clock — **behaviourally identical** (`SessionManager::new` already
  delegates to `new_with_clock(SystemClock)`, verified). `InboundServer` adopts via
  `with_coordinator`; the adapter keeps its `Option<String>` cwd-gate root.

**Deliberately deferred (behavior-preserving choices):**
- Boot resume STILL runs via `resume_working_tasks(&server, resume_cap)`;
  `coordinator.resume()` is NOT called — exactly ONE resume path (the switch is
  slice 4, per the double-resume hazard, Fable M4).
- The Coordinator's `allowed_cwd_root` is passed `None` (INERT until a handler
  delegates a cwd-gated op). Parsing `cfg.allowed_cwd_root` at boot would add a new
  failure mode (`SessionCwd::parse` rejects empty/relative roots) that serve never
  had; the real parsed root is wired at the slice that consumes it (batch/detached).
- NO handler rerouted yet — the held `coordinator` is adopted, not dispatched to.

### Slice 2 — batch RPCs delegate through the Coordinator
```
cargo test --workspace -j 1     → 1427 passed / 0 failed / 12 ignored
cargo clippy -p bridge-a2a-inbound -p a2a-bridge -j 1  → clean
```
1427 = slice-1's 1426 + one new test.

**Changes (server.rs):** the four batch handlers (`run_batch_rpc` / `batch_status_rpc`
/ `batch_list_rpc` / `cancel_batch_rpc`) now delegate their terminal call to
`srv.coordinator().{run_batch,batch_status,batch_list,cancel_batch}` when a
Coordinator is present, falling back to the `bridge_coordinator::batch::*` free fn
otherwise. The adapter WRAPPER is preserved verbatim: auth, the `batch_deps(&srv)`
"batch not configured" guard (exact literal), and RunBatch's per-item validation
(non-empty items, item_id default/empty/dup checks, `session_cwd` validation
against the adapter's real cwd-gate root, `task_spec::validate_input`).

**Behavior-preserving argument:** the Coordinator runs on the SAME shared
BatchRuntime + shared detached-deps (task_store/executor/workflows/workflow_cancels/
progress_hubs) adopted in slice 1. RunBatch's second cwd validation inside
`batch::run_batch` runs against the Coordinator's root (`None` in slice 1), but the
adapter's FIRST pass already enforced the real root's `is_under` check and stored a
normalized absolute path — `validate_cwd_str(normalized, None, _)` just re-parses
(no `is_under`), so the result is identical. Malformed cwds are still rejected by
the adapter's first pass with the same error. In all current code `srv.batch.is_some()
⟺ srv.coordinator().is_some()`, so the fallback is a defensive no-op path.

**New test:** `run_batch_rpc_delegates_through_coordinator` (workflow_producer.rs)
builds a server over a real Coordinator (with a `BatchRuntime::new(4,1)` + the
review graph), issues RunBatch (2 items) → asserts a `batchId`; BatchStatus →
same id + `total:2`; BatchList → contains it. Exercises the `Some(coord)` branch.

**Deferred to slice 7:** the adapter still reads `srv.batch` (for the guard) and
`srv.allowed_cwd_root` (for per-item validation); those fields + the real-root
wiring into the Coordinator are the "delete parallel fields" slice's job.

### Slice 3 — read/control-plane → Coordinator
```
cargo test --workspace -j 1     → 1428 passed / 0 failed / 12 ignored
cargo clippy -p bridge-a2a-inbound -j 1  → clean
```
1428 = slice-2's 1427 + one new test.

**Changes (server.rs):**
- `session_inject` → `coordinator.inject(req)` when a Coordinator is present (its
  `session_manager` IS the shared `sm` — identity-proven), else the adapter's `sm`.
  Order preserved (auth → sm-guard → parse → inject); only the terminal call swaps.
- `session_permit` → `coordinator.permit(params)` when present (same shared
  `PermissionRegistry`), else the inline `apply_permit`. `Result<bool>::unwrap_or(false)`
  matches the inline bool (permit never errs).
- **`session_status` NOT delegated** (spec): its wire DTO (`contextId`/`idleAgeMs`/
  `windowFraction`/camelCase caps/`pendingPermissions`) is incompatible with
  `SessionStatusDto`, and `sm.status` is already the shared source — zero dedup.
- **`get_task`/`list_tasks` unchanged:** they already read `srv.task_store`, which
  IS `coordinator.task_store()` after slice 1's adoption. The store-miss WORKING
  heuristic is intact. Slice 7 repoints the field read when it deletes `srv.task_store`.

**Behavior-preserving:** thin pass-throughs to the SAME shared instances (proven by
`with_coordinator_shares_state_identity`). The existing inject/permit tests still
cover the fallback path.

**New test:** `inject_and_permit_delegate_through_coordinator` (server.rs) builds a
warm coordinator-backed server, warms a context, then SessionInject (coordinator
path) → `queued:1` + the SHARED `sm.pending_inject_count == 1`; SessionPermit
(coordinator path) → `resolved:true` + the SHARED registry resolves the rendezvous.

### Slice 4 — detached submit + boot resume → Coordinator (owner live-gate pending)
```
cargo test --workspace -j 1     → 1430 passed / 0 failed / 12 ignored
cargo clippy -p bridge-a2a-inbound -p bridge-coordinator -p a2a-bridge -j 1  → clean
```
1430 = slice-3's 1428 + two new tests.

**Changes:**
- **server.rs (detached submit, unary Workflow arm):** delegates to
  `coordinator.run_workflow(OpParams)` when a Coordinator is present — with
  `agent/model/effort/mode` hardcoded `None` (inv 7 / Fable M1: the arm has ALWAYS
  dropped overrides for workflows, and `run_workflow` REJECTS them; forwarding
  would turn a today-succeeding `a2a-bridge.model` submit into `InvalidRequest`).
  The Working `a2a::Task` response is reconstructed from the returned `TaskId`. The
  existing inline arm stays verbatim as the coordinator-less fallback (tests).
- **main.rs (boot resume):** `resume_working_tasks(&server, cap)` → `coordinator.resume()`,
  REPLACING it (Fable M4 — never both, or a Working task double-spawns two runners).

**Behavior-preserving argument (verified against source):**
- *Submit:* the Coordinator submits over the SAME shared task_store / progress_hubs /
  workflow_cancels / executor (slice 1) and encodes the spec via the SAME
  `encode_workflow_spec` (s8 T9). cwd was already validated in `gate()` against the
  adapter's real root; `run_workflow`'s re-validation (root `None`) is a no-op
  re-parse of the already-normalized path; input re-validation (`validate_input`)
  is the same check the gate already ran. Routed workflows always have a known
  graph, so the unknown-wf branch (adapter finalizes Failed vs Coordinator
  `InvalidRequest`) is unreachable via the router.
- *Resume:* `coordinator.resume()` and `resume_working_tasks` BOTH branch on the
  shared BatchRuntime and dispatch to the identical underlying fns —
  `batch::resume_all` (batch configured) / `detached::resume_non_batch_tasks`
  (else) — over the shared `detached_deps`. `resume_working_tasks` in `detached.rs`
  is a one-line wrapper over `resume_non_batch_tasks`. `allowed_cwd_root` is used
  ONLY in `run_batch`'s submit-time validation (batch.rs:92), never in any resume
  path — so the Coordinator's `None` root is irrelevant to resume. Both use a
  SystemClock. => drop-in equivalent.

**New tests:**
- `resume_interrupts_unresumable_working_task` (coordinator.rs): seeds a `Working`
  task with no snapshot; `coordinator.resume()` scans the store and finalizes it
  `Interrupted` — covers the serve boot-resume entry point deterministically.
- `unary_workflow_submit_delegates_and_strips_overrides` (workflow_producer.rs):
  a unary workflow submit carrying `a2a-bridge.effort/model` overrides still returns
  a Working task via the Coordinator (not `InvalidRequest`) — proves the strip.

**OWNER LIVE-GATE PENDING (task #22):** `cargo test` cannot cover a real
boot→submit→restart→resume cycle. Owner must run: submit a detached workflow →
restart serve → confirm it resumes from the durable store (cross-surface s8 T9).

### Slice 5 — context-lifecycle → Coordinator (owner force-reset live-gate pending)
```
cargo test --workspace -j 1     → 1431 passed / 0 failed / 12 ignored
cargo clippy -p bridge-a2a-inbound -p bridge-coordinator -p bridge-mcp -p a2a-bridge -j 1  → clean
```
1431 = slice-4's 1430 + one new test.

**Changes:**
- **coordinator.rs:** `clear(ctx)` → `clear(ctx, force: bool)` (was hardcoding
  `false`); passes `force` to `clear_with_children`. `force = true` aborts an
  in-flight warm turn instead of rejecting.
- **bridge-mcp/server.rs:** the one `coord.clear(ctx)` caller → `coord.clear(ctx, false)`
  (mcp SessionClear has no force flag → non-force clear, behaviour unchanged).
- **server.rs `session_clear`:** early-return delegation to `coordinator.clear(ctx, force)`
  when a Coordinator is present; the inline arm stays verbatim as the coordinator-less
  fallback. Identical by construction: the Coordinator holds the SAME shared
  `workflow_runs` busy-guard + `session_manager` (slice 1), same lock scope
  (lock → busy-check → `clear_with_children(force)` → drop), same response mapping.
- **`session_release`/`session_compact`/`session_cancel` NOT changed (D4):** they
  already call `srv.session_manager` (= `coordinator.session_manager`) + the shared
  `srv.workflow_runs` — i.e. they ALREADY operate on the Coordinator's instances.
  There is no Coordinator method for them; slice 7 repoints the field *read* when it
  deletes `srv.session_manager`. `session_compact`'s detached-task-so-a-dropped-caller-
  can't-strand-`Compacting` guard is untouched.

**New test:** `session_clear_delegates_through_coordinator` (server.rs): warm a
context, then SessionClear `force:true` through the coordinator → `cleared:true` +
a bumped generation on the shared session_manager.

**OWNER LIVE-GATE PENDING (Fable M5, task #23):** `clear(force=true)` fires an
in-flight warm turn's abort token (both biased selects) — `cargo test` can't drive a
real mid-turn force-reset. Owner must run: force-reset a context WITH a warm turn
in flight and confirm the turn aborts cleanly.

### Slice 6 — warm turn / cancel: MINIMAL delegation (NO CODE CHANGE)
Slice 6 is a **confirmation slice**, by design (D3 + inv 5/8). The warm Local send
arm (streaming AND unary) and every `cancel_task` arm (durable / delegation / fanout
/ local) STAY adapter-resident — they carry A2A-wire semantics (client task-id, SSE,
disconnect, terminal-echo, the get_task WORKING heuristic) that the MCP-shaped
`Coordinator::prompt` / `Coordinator::cancel_task` do NOT model, and delegating would
regress them (Fable B2 / codex: `prompt` mints a synthetic id and is collect-only;
`cancel_task` sets `canceled` without `terminal_seq`).

After slice 1 these handlers already operate on the SHARED `session_manager` /
`workflow_cancels` / `workflow_runs` / `store` / `task_store` / `bindings` / `registry`,
so nothing needs to delegate. Verified: `grep` confirms NO
`coordinator.prompt`/`coordinator.cancel_task`/`continue_turn` call in server.rs — the
arms read the shared `srv.*` fields directly.

**Guarded by** `warm_unary_cancel_by_wire_id_hits_real_session` (the harness gap test
added FIRST this session): a warm unary turn cancelled by wire task-id fires
`backend.cancel` on the REAL warm session, not the synthetic `session-{task}` fallback
— exactly the regression that delegating to `coordinator.prompt` would cause. Plus the
existing warm/cancel suite (`concurrent_same_context_workflow_handle_busy`,
`session_cancel_cancels_workflow_run`, `cancel_task_fires_workflow_token_stream_ends_canceled`,
`cancel_task_propagates_to_backend`, fanout cancels) — all green.

**OWNER LIVE-GATE PENDING (task #24):** warm multi-turn + mid-turn
cancel-while-running (durable arm) + delegation/fanout round-trip.

## Verified
- Full workspace suite green (**1431/0/12** after slices 5 & 6; 1430 after slice 4;
  1428 after slice 3; 1427 after slice 2; 1426 after slice 1) on the final tree;
  clippy clean; golden-wire 15/15.
- Slices 1–6 are STATE-sharing + delegation of the pure-duplicate/stateless RPCs;
  the warm/streaming/cancel wire paths stay adapter-resident over shared state.
- Shared-Arc identity proven by `Arc::ptr_eq` across all 13 shared fields.
- SessionManager clock switch proven behavior-identical from source.

## Not verified (pending)
- **Owner-run A2A LIVE-GATE for slice 1:** serve boots + a send/receive round-trips
  (the in-process harness covers the router but not a real socket + real agent).
- Slices 2–7 (batch RPCs → read/control-plane → detached submit+resume →
  context-lifecycle → warm/cancel minimal → delete parallel fields).

## Out-of-scope failures
- None. Every run showed 0 failures; nothing re-baselined or silently fixed.
