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

## Verified
- Full workspace suite green (**1427/0/12** after slice 2; 1426 after slice 1) on
  the final tree; clippy clean; golden-wire 15/15.
- Shared-Arc identity proven by `Arc::ptr_eq` across all 13 shared fields.
- SessionManager clock switch proven behavior-identical from source.

## Not verified (pending)
- **Owner-run A2A LIVE-GATE for slice 1:** serve boots + a send/receive round-trips
  (the in-process harness covers the router but not a real socket + real agent).
- Slices 2–7 (batch RPCs → read/control-plane → detached submit+resume →
  context-lifecycle → warm/cancel minimal → delete parallel fields).

## Out-of-scope failures
- None. Every run showed 0 failures; nothing re-baselined or silently fixed.
