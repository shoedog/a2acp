You are doing a focused, adversarial RE-REVIEW (read-only) of the REVISED implementation plan for "E3 — Parallel Batch
Dispatch" for the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). A first dual plan-review
found 10 BLOCKER + 8 MAJOR (mostly anchor/API-shape errors); all were folded into the plan's **`## v2` section
(BINDING)** as PR-FIX-1..17. YOUR JOB: verify each PR-FIX actually RESOLVES its finding *and compiles against the real
code*, and hunt NEW issues the v2 restructures introduce — especially the admission-loop reshape + the resume
extraction. READ-ONLY: read the plan + the binding spec + the real code with read-only tools; do NOT edit/build/test.
Be terse; end with a bounded STOP.

- PLAN: `docs/superpowers/plans/2026-06-26-e3-batch.md` — read the **`## v2` section FIRST** (BINDING, supersedes the
  v1 task snippets), then the v1 tasks for context.
- BINDING SPEC: `docs/superpowers/specs/2026-06-26-e3-batch.md` (`## v3` RR-FIX-1..14 + `## v2` SR-FIX-1..17).

E3 = `run-batch <workflow> --manifest <file>` → N independent detached workflow runs under a serve-wide `Semaphore`
cap, durable `batch` parent record (status/cancel/crash-safe tail-resume), each child via `spawn_detached_workflow`;
a `claim_batch_child` store primitive owns spawn-ownership; the admission loop + resume + roll-up are free fns in
`bridge-coordinator::batch` over a `BatchDeps` bundle, reached via a `batch_deps()` on BOTH `InboundServer` (serve)
and `Coordinator` (mcp).

The v2 folds to validate (RESOLVED / PARTIALLY-RESOLVED / NOT-RESOLVED for each, with a real `file:line`):
- **PR-FIX-1** `TaskRecord` has no serde derives (task_store.rs:59) → `batch_id`/`item_id` plain fields.
- **PR-FIX-2** real test helper `rec(...)` (task_store.rs:823), not `sample_working_task_record()`.
- **PR-FIX-3** `pub mod batch;` + `pub use` (cross-crate from bridge-a2a-inbound).
- **PR-FIX-4** `SqliteStore::open`/`open_in_memory` are SYNC (sqlite.rs:25/41); `tempfile` is NOT a dev-dep
  (bridge-store/Cargo.toml).
- **PR-FIX-5** `RegistryConfig::parse`(919)/`into_snapshot`(1023), required `default` field (119), no
  `validate()/normalized()`; `BatchToml::to_config()` per the `ImplementToml::to_config` precedent (config.rs:778).
- **PR-FIX-6** `default_concurrency: Option<u32>` (omit→default, explicit 0→reject at load).
- **PR-FIX-7** extract `detached::resume_non_batch_tasks(deps,cap)` (the call-site filter is impossible through the
  delegating adapter `resume_working_tasks` server.rs:2155 → detached.rs:1423 which queries `working_tasks()`).
- **PR-FIX-8** batch resume threads the W3b poison cap (`claim_resume_attempt`/`resume_attempt_cap`, detached.rs:1573)
  + reuses the existing per-child resume logic (detached.rs:1602-1638).
- **PR-FIX-9/10** lower-level `run_admission(deps, bid, pending, inflight, concurrency, token)` +
  `InflightChild { task_id, join, _permit }`; drain-only on cancel (no biased-select starvation); cancel each once.
- **PR-FIX-11** after `claim→Created`, re-check BOTH batch (`Canceling`) AND the child row (`cancel_task` flips a
  working child with no token, server.rs:2733).
- **PR-FIX-12** named per-child mechanics: `encode_workflow_spec(graph)`; insert ORDER hub→token→spawn; resume via
  `executor.run_from(seed)`; `claim_batch_child` replaces `create` as the INSERT.
- **PR-FIX-13** `list_tasks` is an inline `json!` (server.rs:3174), not `TaskStatusDto` → add `batch_id`/`item_id` there.
- **PR-FIX-14** T6 Step-0 barrier-gated harness (new scaffolding; no precedent in bridge-coordinator).
- **PR-FIX-15** `ConfigInvalid` is wire-redacted (error.rs:99-100) → the `RunBatch` arm emits a JSON-RPC error
  directly with the item id.
- **PR-FIX-16** `validate_cwd_str(s, root, field)` (the real `OpParams::validate_cwd` params.rs:263 takes no `s`,
  returns `Option`; the A2A path is `session_cwd_from_params` server.rs:3327, label `a2a-bridge.cwd`).
- **PR-FIX-17** `batch_deps()` normalizes per-call.

{{input}}

GROUND every finding in a real `file:line`. Pressure-test the V2 RESTRUCTURES specifically:
1. **Does the corrected code now actually COMPILE against the real signatures?** Spot-check the riskiest:
   `BatchToml::to_config()` + `into_snapshot()` wiring (does `into_snapshot` return `Result<RegistrySnapshot,
   ConfigError>` and is `ConfigError::Registry(String)` the right reject variant?); `SqliteStore::open_in_memory()`
   sync call in a `#[tokio::test]`; `pub use batch::…` (do all named items exist after T6/T7?); the `list_tasks`
   inline-`json!` edit reading `r.batch_id` (an `Option<BatchId>` — does `BatchId` expose `as_str`?).
2. **PR-FIX-9/10 admission reshape — is it actually correct + faithful?** Does `run_admission(pending, inflight)`
   compose for BOTH T6 (empty inflight) and T7 (seeded inflight)? Does `InflightChild { task_id, join, _permit }`
   keep enough to (a) cancel the right child once and (b) free the permit on completion? Is the "drain-only on cancel"
   loop correct (does it still free permits + reach the settle), and does it actually fix the starvation (the cancel
   arm no longer competes with `inflight.next()`)? Any NEW deadlock/leak from carrying `_permit` in the struct vs the
   future?
3. **PR-FIX-7/8 resume extraction — single-owner + poison-cap intact?** Does `resume_non_batch_tasks(deps,cap)` leave
   `detached::resume_working_tasks`'s behavior intact for non-batch while excluding batch children, called from BOTH
   adapters (server.rs:2155 + coordinator.rs:499)? Does threading `cap` into `resume_batches` + reusing the per-child
   resume (detached.rs:1602-1638) actually preserve `claim_resume_attempt` semantics, or does the batch path
   double-count / skip the cap? Is every working task still resumed exactly once across the two routines?
4. **PR-FIX-11 child re-check — closes the gap without a new race?** After `claim→Created`, reading the child row +
   skipping spawn if terminal: is there still a window where `cancel_task` flips the row AFTER the re-check but BEFORE
   spawn (and is that benign — the spawned runner then honors the token)? Does the re-check add a store round-trip per
   admit that could bottleneck under the `Mutex<Connection>`?
5. **PR-FIX-15 item-named reject — does the arm own the response correctly?** Can a `RunBatch` JSON-RPC arm emit
   `jsonrpc_err`/a custom error with the item id WITHOUT routing through `BridgeError` (which would redact)? Is that
   consistent with how the other arms build errors (server.rs `jsonrpc_err`/`bridge_err_to_jsonrpc`)?
6. **New issues from v2.** Did any PR-FIX introduce a NEW undefined type/helper, a forward reference, or a contradiction
   with another PR-FIX or with v3 (RR-FIX-1..14)? Is `BatchConfig` (PR-FIX-5) defined + carried to `BatchRuntime`
   sizing (RR-FIX-12)? Does `Option<u32>` default_concurrency (PR-FIX-6) thread cleanly into `to_config` clamp + the
   per-batch `min(concurrency, max_concurrent)` in `run_batch`? Is the slice still right-sized (did v2 grow T6/T7 past
   one green increment — should the T6 harness or the detached.rs extraction be its own task)?
7. **Still-open.** Any PR-FIX PARTIALLY/NOT resolved. Any remaining wrong `file:line`. Anything that still blocks a
   clean TDD implementation.

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. For each
PR-FIX-1..17 state RESOLVED / PARTIALLY-RESOLVED / NOT-RESOLVED (one line each). End with
`PLAN RE-REVIEW VERDICT: ready-to-implement | needs-revision`. Then STOP.
