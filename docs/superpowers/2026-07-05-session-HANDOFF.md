# Session Handoff — 2026-07-05

**Purpose:** resume cleanly after a compact. This session: ran the M3 eval baseline,
shipped roadmap **#9** (bridge-controller extraction) end-to-end, and took roadmap
**#10** (Coordinator migration) through design + dual-review to an
implementation-ready spec. **RESUME POINT: start #10 implementation** (harness gap
test → slice 1). Read this, then `memory/MEMORY.md`, then the #10 spec.

## Git state at handoff
- **`main` @ `94ea690`** (pushed): #9 merged; M3 baseline merged (`622fe09`).
- **`feat/coordinator-migration` @ `f9a78d0`** (NOT pushed, NOT merged): has ONLY
  the #10 **design record** (spec v2 + both reviews). NO #10 implementation yet.
  **Resume by checking out this branch.**
- Working tree is clean on the branch.

## What shipped to `main` this session

| Commit | What |
|---|---|
| (earlier) `622fe09` | **M3 first measured eval baseline** — 45-run 3-cell, seeded recall 11/11 all cells; precision is the differentiator (codex-solo 2 false findings vs duo 23 / claude-solo 17). PROVISIONAL/n=15; owner C2 spot-check still pending (`evals/results/baselines/2026-07-04-review-seeded-v1/spotcheck.yaml`). |
| `94ea690` | **#9 bridge-controller extraction MERGED** — ~5,400 lines of controller loops → new `crates/bridge-controller` lib (4 ports + pure loops + primitives + VerifyConfig/MergeConfig); bin keeps config-parse + effects adapters + dispatch via one re-export shim. 6 behavior-preserving slices, full suite 1424/0/12 + clippy clean each. Design dual-lens reviewed (Fable caught a security regression codex missed); whole-branch review opus+codex both SHIP. See `memory/bridge-controller-extraction-shipped.md`. |

## #10 — Coordinator migration (IN PROGRESS: design done, implementation NOT started)

**Goal:** make `bridge-a2a-inbound`'s `InboundServer` (server.rs, ~10,381 lines) a
THIN ADAPTER over `Arc<bridge_coordinator::Coordinator>`, deleting its PARALLEL
lifecycle-state instances so A2A is co-equal with CLI/MCP. The analysis's "one
incomplete architectural ruling." L effort / **M-H risk** (warm-path cancel/binding
invariants are bug-prone).

**Owner-chosen approach:** Full spec → dual review → sliced. Live-gates handled by
**building a live test harness FIRST** (owner picked this over manual gates). Start
slice 1 now (after the harness gap test).

**Spec (v2, IMPLEMENTATION-READY):** `docs/superpowers/specs/2026-07-05-coordinator-migration.md`.
Reviews (both REVISE, folded): `docs/superpowers/specs/reviews/2026-07-05-coordinator-migration-{codex-xhigh,fable-review}.md`.
Full state + decisions also in `memory/coordinator-migration-10-design.md`.

**Key decisions (post-review — DO NOT re-derive):**
- **D1** — instance-share the ONE in-memory SessionStore (pass the adapter's `store`
  Arc into `Coordinator::new` as session_store). NOT two instances (split-brain),
  NOT file-backed (durable cancel/fanout latches on reusable ids kill fresh tasks).
- **D2** — build the Coordinator FIRST (it already owns the 4 maps, `coordinator.rs:150`),
  adapter ADOPTS the SAME Arcs via new **`pub`** accessors. `pub(crate)` is INVALID
  (separate crates). No `Coordinator::new` variant; `mcp` path untouched.
- **D3** — the live-STREAMING arm stays adapter-resident (`run_workflow` is
  detached-only). "Co-equal = one lifecycle-STATE owner, NOT method parity." The
  warm/cancel/status handlers stay adapter-resident WRAPPERS over shared state; only
  STATE + stateless RPCs (inject/permit/batch) + detached submit + boot resume +
  read-plane delegate.

**Slice-1 shared-identity set (all SAME instances):** 4 maps (bindings,
workflow_cancels, workflow_runs, progress_hubs) + task_store + session_manager +
permission_registry + **registry** (hot-reload applies to ONE Arc) + **ONE
BatchRuntime** (built twice today at main.rs:4831/6109 → 2 semaphores would double
the serve cap) + **store** (D1).

**MUST-PRESERVE invariants (don't delegate naively):** terminal_seq — do NOT route
A2A live cancel to `Coordinator::cancel_task`; STRIP agent/model/effort/mode
overrides before `run_workflow`; boot resume must REPLACE `resume_working_tasks`
(never both); biased abort-select in both warm drains; `clear(force=true)` fires
warm aborts (live-gate at slice 5).

**7 slices:** 1 one-Coordinator+adopt-shared-set → 2 batch RPCs → 3 read/control-plane
(NOT session_status — wire-incompatible DTO) → 4 detached submit+resume (strip
overrides, exclusive replace) → 5 context-lifecycle (clear(force)/release/compact/
cancel) → 6 warm/cancel MINIMAL (Local arm STAYS adapter-resident, cancel_task
durable arm stays a wrapper) → 7 delete parallel DELEGATE fields, keep 8 KEEP +
Arc<Coordinator>.

## THE EXACT NEXT STEPS (resume here)

1. **Harness gap test** (task #17). A rich in-process A2A harness ALREADY exists:
   `crates/bridge-a2a-inbound/tests/workflow_producer.rs` — `build_workflow_server()
   -> Arc<InboundServer>` + drive traffic via `srv.router().oneshot(req)` (axum, no
   sockets); it already pins terminal-seq, both CancelTask variants, warm-workflow
   cancel races, resume, and the unary `status`-chunks shape (golden_wire.rs:541).
   The ONE gap: a warm **UNARY (Local, non-workflow)** turn cancelled by wire
   task-id mid-turn, asserting the REAL warm session is cancelled (the behavior that
   breaks if slice 6 delegates to `coordinator.prompt`'s synthetic id). Add it +
   verify SessionStatus wire-shape is pinned. Model it on
   `cancel_task_fires_workflow_token_stream_ends_canceled` (workflow_producer.rs:1417)
   but for a warm local send.
2. **Slice 1** (task #18): add the `pub` accessors on `Coordinator`; build ONE
   `Coordinator` (+ one `BatchRuntime` + shared `store`) in the serve path
   (`bin/a2a-bridge/src/main.rs` ~6004-6128); `InboundServer` holds `Arc<Coordinator>`
   and adopts the SAME identity set — no handler reroute yet. Add a shared-Arc-identity
   test (assert `Arc::ptr_eq` between adapter's handle and coordinator's). Green on
   suite + golden-wire; live-gate = A2A boot + send/receive (via the harness).

## Process learnings (apply on resume) — all in `memory/review-agent-roles.md`
- **Dual-lens design review (codex xhigh + Fable) is worth it on one-way-door
  decisions.** This session Fable caught the #9 security regression AND the deeper
  #10 defects (BatchRuntime doubling, store split-brain) that codex missed; codex
  caught what Fable missed. Fable needs owner vetting (done for #9 + #10) and can't
  go through the bridge (blocklisted) — dispatch as an Agent-tool sub-agent
  `model: "fable"`.
- **Harness `<new-diagnostics>` mid-edit snapshots are STALE** (fired phantom errors
  on 3 of #9's 6 slices; rustc was clean each time). Verify sub-agent results with
  `cargo build`/`cargo test` from scratch — trust neither the agent's "all green"
  NOR the phantom diagnostics.
- **Model policy for implementation:** sonnet-medium does the mechanical slices; the
  orchestrator PRE-SCOPES each to an exact recipe (the real anti-thrash lever) +
  independently verifies the full suite; escalate to codex-high only on genuine
  defects (none needed for #9).
- **`cargo test --workspace -j 1`** (the `-j 1` avoids the known linker OOM); full
  suite is 1424/0/12 on `main`.

## Roadmap position
Done: identity/license, Waves 1-3, M2 release, M3 baseline (spot-check pending), **#9**.
In progress: **#10** (design done, implementation next). After #10: the medium/
long-term ideas in `docs/2026-07-03-strategic-analysis.md` (M1 composable isolation,
L1 A2A federation, L9 workflow packs, etc.).
