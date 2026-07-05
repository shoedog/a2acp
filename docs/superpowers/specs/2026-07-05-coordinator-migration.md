# Coordinator migration — architecture spec v2 (roadmap #10)

**Status:** IMPLEMENTATION-READY. v2 folds the dual review (codex gpt-5.5 xhigh +
Fable), both verdict REVISE. Both confirmed the architecture is sound (one
Coordinator, shared-identity Arcs, adapter keeps wire concerns; D1/D3 point the
right way) but v1 shipped wire changes and had a cross-crate visibility bug. All
findings dispositioned in §Reviews; the load-bearing new claims (B1 store, B3
BatchRuntime, D2 inversion) were re-verified against source.
**Grounded in:** the full `InboundServer`↔`Coordinator` seam map.
**Date:** 2026-07-05.

## Goal & non-goals

**Goal.** Finish slice-8's ruling (`coordinator.rs:93`): make `InboundServer` hold
`Arc<Coordinator>` over the **same** lifecycle-state instances, delete its
*parallel* copies, and delegate the handlers that are pure duplicates. So
turn-lifecycle **STATE** has one owner.

**Honest scope (Fable):** "ships once" holds for STATE, not for the streaming
**drain code** — `spawn_local_producer`'s drain loop stays a parallel
implementation of `collect_turn`'s warm protocol (biased select + usage +
finish-guard). A2A's entire *streaming* surface (turns AND workflows) remains
adapter-resident. **"Co-equal" = one lifecycle-state owner, NOT method parity.**

**Non-goals.** Behavior-preserving — no A2A wire change (golden-wire + a live
gate must stay green). Not an A2A-wire rewrite. No new capability. Does not unify
the two serve commands (`mcp` stdio stays untouched); only the *state* converges.

## The revised shape

Delegation is clean for **STATE + stateless RPCs**: `inject`→`coordinator.inject`,
`permit`→`coordinator.permit`, the four batch RPCs, detached-workflow submit →
`coordinator.run_workflow`, boot resume → `coordinator.resume`, read-plane over
the shared `task_store`. The **warm/streaming/cancel/status** handlers stay
adapter-resident as thin wrappers over *shared* Coordinator state — they carry
A2A-wire semantics (client task-id, SSE, disconnect, terminal-echo, DTO shape)
that the MCP-shaped Coordinator methods do not model.

## Decisions (revised)

### D1 — Instance-share the ONE store (not two instances). [codex/Fable B1]
The adapter has exactly ONE `store: Arc<dyn SessionStore>` serving BOTH lifecycle
(`store.put(task, session)` before every dispatch; translator `put_pending`) AND
delegation/fanout/cancel bookkeeping (`set_peer_task`/`request_cancel`/`is_fanout`).
v1 said "adapter keeps its own delegation store; only the lifecycle store
converges" — that would **split-brain** task→session state across two stores.
**Decision: pass the adapter's ONE existing in-memory store Arc into
`Coordinator::new` as `session_store`.** D1's conclusion survives — no durability
flip (stays in-memory) — but the mechanism is instance-*sharing*, not two
instances. **Do NOT file-back it** (Fable): durable `cancel_requested`/`is_fanout`
latches keyed by reusable ids would instantly kill fresh post-restart tasks that
reuse an id (`server.rs:1677`, `3535`). The in-memory loss is a narrow latent gap
(post-restart peer-cancel no-ops a synthetic session), acceptable and out of scope.

### D2 — Build Coordinator FIRST; adapter ADOPTS the same Arcs via PUBLIC accessors. [codex B1 / Fable M3b + D2]
v1 said `pub(crate)` accessors — **impossible**: `InboundServer`
(`bridge-a2a-inbound`) and `Coordinator` (`bridge-coordinator`) are separate
crates, so `pub(crate)` is invisible. And v1's "inject shared Arcs into
`Coordinator::new`" would create a second construction shape for the state owner —
the exact divergence class this migration kills. **Decision: `Coordinator::new`
already constructs+owns the four maps (`coordinator.rs:150-153`); build the
Coordinator FIRST in the serve path, then the adapter adopts the SAME instances
via new `pub` accessors** — `task_store()`, `registry()`, `policy()`,
`bindings()`, `workflow_cancels()`, `workflow_runs()`, `progress_hubs()`,
`permission_registry()`, `executor()`, `workflows()`, `allowed_cwd_root()`,
`batch()`. No `Coordinator::new` variant; the `mcp` path stays byte-untouched.

### D3 — Live-streaming stays adapter-resident (confirmed detached-only). [both]
`Coordinator::run_workflow` is detached-only (`coordinator.rs:392`); the streaming
machinery (`spawn_workflow_producer`, `WarmWorkflowNodeDispatcher`, the
`workflow_runs` admission) is A2A-only. Kept adapter-resident, driven via the
shared `executor()`/`workflows()`/`workflow_cancels()`. Per the honest-scope note,
this extends to the streaming *turn* arm too.

### D4 — Context-lifecycle ops → `coordinator.session_manager` (already `pub`). [unchanged]
`session_release`/`session_compact`/`session_cancel` (context-keyed) have no
Coordinator method → adapter calls `coordinator.session_manager.{release_with_children,
compact_session, cancel_with_children}` + the shared `workflow_runs()` busy-guard.
`session_clear` DOES delegate to `coordinator.clear(force)` once a `force: bool`
param is added (it hardcodes `false`, `coordinator.rs:500`).

## The slice-1 shared-identity set (all the SAME instances)

Built once, owned by the Coordinator, adopted by the adapter: the four maps
(`bindings`, `workflow_cancels`, `workflow_runs`, `progress_hubs`) + `task_store`
+ `session_manager` + `permission_registry` + **`registry`** (the hot-reload
reconcile loop applies snapshots to ONE Arc, `main.rs:5986` — Fable) + **one
`BatchRuntime`** (built twice today at `main.rs:4831`/`6109`; two semaphores would
double the serve-wide cap + orphan `CancelBatch` on boot-resumed items — Fable B3)
+ the **`store`** (D1). Genuine dual-writers are `workflow_cancels`/`progress_hubs`/
`workflow_runs`; `bindings` is adapter-only today (Coordinator's touch is the dead
`_deferred_cold_bindings` placeholder, `coordinator.rs:231`) but sharing it is
forward-correct.

## Field disposition (21 = 9 DELEGATE + 4 shared-Arc + 8 KEEP)

- **DELEGATE (drop the adapter's copy, read via Coordinator/accessor):** registry,
  policy, executor, workflows, task_store, session_manager, permission_registry,
  allowed_cwd_root (via accessor — it's cwd-gate state, KEEP-or-accessor), batch.
- **Shared-Arc (same instance both sides):** bindings, workflow_cancels,
  workflow_runs, progress_hubs.
- **KEEP (8, adapter-only wire concerns):** route, auth, base_url, delegation,
  local_source_label, cancelled_peers, model_catalog, **`store`** (v1 wrongly
  omitted it — it's the delegation/fanout/cancel-latch store; it is ALSO shared
  into Coordinator per D1, but the adapter keeps the handle for its wire paths).

## Invariants to preserve (§C + the review additions)

1. Bindings bind-before-spawn + `BindingGuard` eviction (leaked lease strands a
   registry slot).
2. `cancelled_peers` single-cancel + BOTH cancel paths (double-cancel otherwise).
3. `workflow_cancels` remove-AFTER-terminal (`bridge_coordinator::detached` — both
   surfaces already call it, `server.rs:2143`).
4. s8 T9 `encode_workflow_spec` non-divergence (delegating detached submit
   satisfies it).
5. Biased abort-select in BOTH warm drain loops (`server.rs:2401`,
   `coordinator.rs:320`).
6. **[Fable B3] E3 one serve-wide `BatchRuntime` semaphore** + cross-batch admit.
7. **[Fable M1] `run_workflow` override rejection:** `Coordinator::run_workflow`
   errors on `agent/model/effort/mode`; the A2A arm silently DROPS them
   (`server.rs:416`). Slice-4 wrapper MUST strip overrides before delegating, or
   today-succeeding `a2a-bridge.model` submits become `InvalidRequest`.
8. **[codex/Fable M2] Terminal sequencing:** A2A cancel fires the token and lets
   the runner write the *sequenced* terminal; `Coordinator::cancel_task` fires +
   immediately `cancel_if_working` (`status='canceled'`, `terminal_seq=NULL`) — a
   subscriber can snapshot a non-sequenced terminal. The durable cancel arm stays
   an adapter wrapper (token-fire-early-return + true-state re-read), NOT delegated.

## Slice plan v2 (behavior-preserving; each green on suite + golden-wire; live-gated where noted)

1. **One Coordinator in the serve path; adapter adopts the shared-identity set.**
   Add the D2 `pub` accessors; build ONE `BatchRuntime`+`store`; `Coordinator::new(...)`;
   `InboundServer` gets `Arc<Coordinator>` + adopts the SAME instances (no handler
   reroute). **The full shared-identity set must be identical from this commit** —
   the real constraint is "shared before the first slice calling a Coordinator
   method that touches shared state" (slice 2). Verify: suite + golden-wire; **live:
   A2A serve boots + send/receive.**
2. **Batch RPCs → `coordinator`** (needs slice-1 shared `BatchRuntime`). Preserve
   A2A per-item validation + the literal `"batch not configured"` error (keep the
   wrapper guard).
3. **Read/control-plane:** `inject`→`coordinator.inject`; `permit`→`coordinator.permit`;
   `get_task`/`list_tasks` over shared `task_store()`. **Do NOT delegate
   `session_status`** — its wire DTO (`contextId`/`idleAgeMs`/`windowFraction`/
   camelCase caps/`pendingPermissions`) is incompatible with `SessionStatusDto`
   (`kind`/`idle_age_ms`/`over_threshold`) and delegation buys zero dedup (`sm.status`
   is already the shared source). Keep `get_task`'s store-miss heuristic.
4. **Detached submit + boot resume.** unary Workflow arm → `coordinator.run_workflow`
   **after stripping agent/model/effort/mode overrides** (inv 7). Boot resume →
   `coordinator.resume()` **REPLACING** `resume_working_tasks` (never both — double
   scan double-spawns runners, journal corruption — Fable M4). **Live:** submit →
   restart → resume (cross-surface s8 T9).
5. **Context-lifecycle.** `session_clear`→`coordinator.clear(force)` (add `force`);
   `session_release`/`compact`/`cancel`→`coordinator.session_manager` (D4). Preserve
   `session_compact`'s detached-task-so-caller-drop-can't-strand-`Compacting`.
   **Live-gate MOVED HERE (Fable M5):** `clear(force=true)` fires in-flight warm
   abort tokens (both biased selects) — the force-reset gate must run at this slice,
   not wait for 6.
6. **Warm turn / cancel — MINIMAL delegation.** The Local send arm (streaming AND
   unary) STAYS adapter-resident over the shared `session_manager` (it's already
   thin — `warm_local_dispatch` ~40 lines over `checkout_turn`). **Do NOT delegate
   to `coordinator.prompt`** (it mints its own task-id, is collect-only, no
   disconnect, loses the `status`-chunks — breaks CancelTask-by-wire-id + the
   get_task WORKING heuristic). `cancel_task`'s durable arm stays a wrapper (inv 8);
   its delegation/fanout/local arms stay. **Live:** warm multi-turn + mid-turn
   cancel-while-running (through the durable arm) + delegation/fanout round-trip.
7. **Delete the parallel DELEGATE fields** (registry, policy, executor, workflows,
   task_store, session_manager, permission_registry, allowed_cwd_root, batch, and
   the now-shared four maps), keeping the 8 KEEP + `Arc<Coordinator>`. Final suite +
   golden-wire + live gate.

## Tests to add BEFORE slices 4/6 (both reviews)

Shared-Arc identity assertions; live-token cancel *sequencing* (terminal_seq set
before a subscriber can snapshot); exact `SessionStatus` wire shape; CancelTask
response variants (terminal-echo / durable-arm); the unary-local `status`-chunks
shape. Golden-wire only trips shapes it encodes — these are the exact shapes
slices 4/6 can drift.

## Live-gate plan

Owner-run A2A traffic at: slice 1 (boot + send/receive); slice 4 (submit → restart
→ resume); **slice 5 (force-reset of an in-flight warm turn)**; slice 6 (warm
multi-turn + cancel-while-running + delegation/fanout). Golden-wire runs in CI at
every slice.

## Reviews — every finding dispositioned

Both REVISE; architecture sound; no conflict (Fable extended codex). All verified
against source.
- **D2 `pub(crate)` cross-crate bug** [both] → `pub` accessors, build-Coordinator-first. §D2. ✔
- **D1 two-instance split-brain** [both B1] → instance-share the one store. §D1. ✔
- **slice-7 KEEP omits `store`** [both] → 8 KEEP; field arithmetic fixed. §Field. ✔
- **BatchRuntime doubled** [Fable B3] → one instance in the shared set. §slice-1, inv 6. ✔
- **warm→`prompt` ships wire changes** [both B2/BLOCKER] → Local arm stays adapter-resident. §slice 6. ✔
- **cancel_task terminal-seq race** [both M2] → durable arm stays a wrapper. inv 8. ✔
- **run_workflow override rejection** [Fable M1] → strip before delegating. inv 7, §slice 4. ✔
- **double boot-resume** [Fable M4] → REPLACE, exclusive. §slice 4. ✔
- **clear(force) fires warm aborts pre-gate** [Fable M5] → live-gate moved to slice 5. ✔
- **session_status wire-incompatible** [both] → don't delegate. §slice 3. ✔
- **bindings not dual-mutated / registry identity / clock asymmetry** [both] → §slice-1 set, MINOR. ✔
- cite drift (`run_workflow` :392) fixed.

**No open design questions.** The one genuine disagreement class (delegate vs
adapter-resident for warm/cancel/status) was resolved against source in the
reviewers' favor: those paths stay adapter-resident over shared state.
Implementation may proceed on the seven-slice plan.
