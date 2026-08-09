# R2f1b slice 3 — resource authority (brief, rev 2)

Date: 2026-08-09. Author: Fable orchestrator (session 9337e035). Base for every
measured claim: origin/main `f58862a5` (slice 2 complete; workspace 3826/0/12
across 90 harnesses — fold-gate totals live in each sub-slice's dual-review
record and the slice-2 handoff ledger, which is this program's declared home for
them). Authority chain: focused boundary
(`2026-08-02-r2f1b-focused-boundary.md`) §§5.3, 5.4, 5.7 rows 7–11, §6, §7 item
3; custody plan (`2026-08-06-r2f1b-pre-slice-2-custody-plan.md`) §§4, 9; the
slice-2 review-record ledgers (s2b2/s2c1/s2c2/s2d); owner rulings of 2026-08-09
(Candidate settlement → slice 3; two-phase settlement ratified-as-shipped).

**Rev 2 change log (dual design review, one declared round, adjudicated).**
Rev 1 was reviewed by an opus senior-lead lens (REVISE: 6 WRONG, 8 SMELL) and
the bridge `plan-review` workflow (REVISE: 30 BLOCKER, 21 MAJOR, 4 MINOR).
Folded: the inverted `process.rs` drop reconciliation (opus W-1 / B1 — rev 1's
own measurement was wrong; the boundary was exact); `WorktreeBackend` as the
unowned fifth adapter (W-2/B2); the resequenced, resized split (W-3/B30 — the
coverage lens measured slice 2's real multiplier at median 3.2×, refuting rev
1's 2–3× and its STOP rule, which would have fired on 4 of 6 folds); epoch
linearization pulled ahead of the sub-slices that fire its trigger (W-4/M4);
the exact error-exit population (W-5/B24: symbol
`run_from_with_context_inner`, 21 `yield Err` sites, 10 post-recording); the
custody-plan §9 carry rows (W-6/M6); the §6-matrix ownership table with the
boundary's verbatim test names (S-2/M2); the row-10 clock closure via the
injected `MonotonicClock` (S-1) and the finite-preparation row via
`PreparationClockV1` (M3); row 11 reassigned wholly to slice 5 (M5 — landed
slice-1 coverage makes its slice-3 half vacuous); plus every seam CONSTRAINT
the reviews surfaced (sync-joinable journal B13, settle-before-yield B23,
platform start-identity probes B3, flight-before-spawn B11, injectable
authority ports B10, typed container spawn result B14, request-vs-session
identity B15, recovery seam B18, settlement lock window B19, descriptor-safe
marker unlink B20, preparation terminal-state check B21, attach-vs-discover
semantics M12, labels-digest canonicalization M16, V2 controls + the
universal-vs-gated decision B17, teardown-entrance enumeration B16/B12).
**Adjudicated to the lane's layering, recorded not silently dismissed:** the
review class "write the full API signatures into this brief" (B5/B7/B8/B25 and
kin) — this program's briefs mandate constraints, red tests, and
design-note/dispatch-brief obligations; implementers design the APIs and the
dual lens reviews them. Every such finding is folded as a NAMED constraint plus
a mandatory design note; none is dropped.

**Mandate (boundary §7 item 3, verbatim):** "Resource authority.
`OwnedProcessTree`, generation flights, five backend adapters, `ReapController`
integration, registry/resilient/session wrappers, collateral journal.
Unrelated-process-survival and one-flight tests green." Plus the carried
obligations in §1.3. Slice 4 may not enable any deadline before slices 1–3 are
green (§7), so §6 here is a hard precondition for the scheduler.

**The safety direction, stated once:** slice 2 made *checkout* destruction
capability-gated. Slice 3 does the same for *process/container* destruction: no
signal without a journaled, admission-closed, capability-bound flight. Today
signals fire from raw `Drop`s and retirement paths with no journal and no owner
set — every such path must come to join or refuse.

---

## 1. Measured reconciliation (live `f58862a5` — trust these over older anchors)

### 1.1 Already landed / discharged (do NOT re-mandate)

| Item | Status on main |
|---|---|
| Pre-slice-3 `deny_unknown_fields` on `PreparationFlightStateV1`/`ResourceFlightStateV1` | DONE (F-2; `preparation_flight.rs:114`, `resource_flight.rs:171`; `Open {}` forms; A2 goldens hold) |
| `AutomaticR2f1b` production refusal | DONE (2b2 both-entrance refusal) |
| Flight TYPE contracts | LANDED, zero production writers — slice 3 writes the first producers |
| §5.7 rows 1–2, 4–6, 12 | green by name at the 2d fold. **Row 3 precision (M20):** its sweep-exclusion half is green; its "only exact proof of no materialization may remove the unused marker" half is BEHAVIORALLY OPEN until 3d (no `UnusedSettled` producer exists) — the slice-2 exit ledger's "rows 1–6" claim covers type/transition/exclusion level only for row 3 |
| Row 11 (crash after primary commit → resume preserves primary, cleanup CAS only) | Slice-1 coverage already pins the CAS mechanics (`v3_primary_and_pending_cleanup_are_atomically_reserved`; `settle_node_cleanup_v3` apply/replay/conflict tests); the missing half is the production RESUME driver — **row 11 is wholly slice 5's, declared** (M5) |
| `AgentBackend` custody surface | `configure_bound_session` (:358), `preserve_checkout_v1` (:428), `settle_workflow_checkout_v1` (:463) exist, all defaulted; only `prompt`/`cancel` required |

### 1.2 Anchor and surface facts (measured; rev 1's drop claim CORRECTED)

- **`Supervised` has exactly THREE signal paths, and the boundary's §5.4 anchor
  is CONFIRMED EXACT** (rev 1 inverted this — its probe read `terminate`'s
  lines as the drop): `impl Drop` at `process.rs:530-546` is a **single-stage,
  unconditional, non-blocking `libc::kill(-pgid, SIGKILL)`** (`:543`);
  `terminate(grace)` at `:403-422` is the async two-stage SIGTERM→wait→SIGKILL
  path; **`terminate_blocking` at `:428-475`** is a blocking two-stage variant —
  called from `acp_backend.rs:2130` inside `UnpublishedContainerCleanup::start`,
  which runs on a **bare `std::thread` with no Tokio runtime** (B13). All three
  must join-or-refuse; the third forces 3a's runner to be joinable from a
  non-async context.
- `ReapController` `reaper.rs:78`; `escalate_terminate` `acp_backend.rs:5160`;
  `wait_for_slot_drain` `registry.rs:533`; `ResilientWarm` retire sites
  `resilient.rs:71/:178/:192`; production spawn `acp_backend.rs:3119` (plus
  direct `Supervised::spawn*` uses in `bridge-acp` tests/helpers and three bin
  integration tests — 3b1 enumerates and migrates ALL of them, B6).
- Five production `AgentBackend` impls: `acp_backend.rs:6485`,
  `bridge-api/src/backend.rs:644`, `bridge-container/src/lib.rs:1298`,
  `bridge-worktree/src/backend.rs:2596`, `replay.rs:60` (refuses V3). ~120
  impls total (39 executor, 24 a2a-inbound server, 11 coordinator, 11
  workflow_producer, rest scattered); the V3-path double population is
  enumerated from source at 3b2 dispatch, never estimated.
- `ProcessStartIdentityV1.start_time_ticks` (`resource_flight.rs:80-84`) is
  Linux-shaped (`/proc/<pid>/stat` field 22) and has **zero producers**; darwin
  has no `/proc` and the repo's only `sysctl` use is args-not-start-time. The
  slice gate depends on this probe existing on BOTH platforms (B3).
- Error-exit population: `WorkflowExecutor::run_from_with_context_inner`
  (`executor.rs:4628-5483`); **21** `yield Err(…); return;` sites; the post-loop
  settlement pass at `:5455-5476`; **10 in-scope** (post-checkout-recording:
  5083, 5128, 5212, 5220, 5231, 5257, 5263, 5363, 5381, 5400 — re-enumerate at
  dispatch, these drift); 11 pre-scheduling validation exits stay effect-free.
  `record_checkout` sites: `executor.rs:2056`, `:3499`.
  `deletion_generation_is_current` `bridge-worktree/src/backend.rs:1300`
  (checked at `:1819`). `materialize_under_custody` `backend.rs:2205`.
- `UnusedSettled` has exactly ONE entry edge in `LEGAL_CUSTODY_TRANSITIONS_V1`:
  `(ProtectionPrepared, UnusedSettled)` (`custody.rs:393`).
- Dispatch config: `examples/a2a-bridge.r2f1b-impl.toml` is COMMITTED with this
  brief (B4) — terra/xhigh impl agent + the hermetic verify skip list including
  the 2c2 flock/exec-family exclusions (already present in the file, not
  assumed).
- The roadmap cursor (`docs/reliability-execution-roadmap.md`) was stale by all
  of Track A and slice 2; reconciled in this brief's landing commit, and EVERY
  slice-3 fold updates it (M21).

### 1.3 Carried obligations that BIND slice 3

1. **Candidate settlement (owner ruling 2026-08-09).** Recovery-side
   `UnusedSettled` producer + `unused_candidate_settles_only_after_exact_absence`
   (§5.7 row 3's marker half). Scope: the `(ProtectionPrepared, UnusedSettled)`
   edge population ONLY. → 3d
2. **The 2b2 pre-target add-failure marker population (M8).** Those markers
   retain `PreservationUnknown` (terminal; NO table edge to `UnusedSettled`;
   the 2026-08-09 owner ruling made the retention final with the remedy named
   as "a narrowly-scoped marker-removal authority, NOT a table edge"). 3d
   serves it with a marker-removal authority keyed on state-agnostic exact
   absence proof — a removal API, not a transition. The frozen table is a wall
   3d must NOT rediscover in-round.
3. **Claimed, non-cancellable materialization flight** (2c1 ledger; §2.5).
   First production writer of `PreparationFlightStateV1`. → 3d
4. **Preparation is finitely owned (M3 — was silently droppable under rev 1's
   no-clock rule):** `PreparationClockV1` (`preparation_flight.rs:127`) already
   wraps the injected `MonotonicClock`; §6 mandates manual-clock tests. The
   bounded-transfer property (`nonreturning_custody_sync_transfers_pre_effect_owner`)
   lands in 3d with ZERO production timers; slice 4 owns arming only. → 3d
5. **Error-exit settlement population** (2c2/2d ledgers). → **3s, FIRST** —
   slice 4's preservation-first cancellation presupposes it, and it must not
   sit behind the riskiest work (M4). Includes B23: settlement runs BEFORE the
   terminal `Err` is yielded (a consumer may stop polling after the first
   error), with a consumer-drops-after-error regression.
6. **Epoch linearization — the trigger fires THIS SLICE.** → 3s, before any
   flight sub-slice folds (W-4/M4). **Declared default: LINEARIZE** the
   generation check with the mint under a shared guard. The "prove the CAS
   suffices" branch is the exception and requires a written mechanism-level
   proof naming the interleavings in the handoff (B26; per the downgrade
   clause, absence of a counterexample is never sufficient) — in that branch
   the barrier tests are green-by-design sufficiency evidence, not red-first.
   Plus the writer-vs-settlement contention test in both orders (M10) — and
   note (M14): those tests exercise the epoch/mint locks, NOT the publication
   cell; the 2c1 blocking-acquisition item stays trigger-gated and untested
   here, explicitly.
7. **Session-manager disposition bookkeeping (R-5)** → 3b2, consolidated with
   the flight trait surface so the ~120-impl population is swept ONCE (S-3).
8. **2c1 composition-invariant trigger (S-4) — FIRES:** 3b/3c change backend
   nesting, perturbing 2c2's "holds by spawn-factory construction" basis. 3b2
   and 3c1 handoffs must restate the composition basis post-change, with a test
   that the defaulted custody/flight methods remain unreachable unforwarded.
9. **`authorize_deletion` record-ownership gap (2c2 SMELL-4, S-5a):** its
   "no constructible failure" basis expires when slice 3 adds concurrent
   callers. → 3s re-verifies the basis under the new concurrency and either
   pins it or escalates.
10. **SIGCHLD residual + the parked descendant-kill investigation** → a bounded
    DIAGNOSTIC PREREQUISITE at 3b1 dispatch (M18): hypotheses, same-environment
    control, an explicit decision record (absorbed-by-tree vs re-parked with
    named owner + observability hooks) BEFORE the process-authority
    implementation gate. These tests are hermetic-container-excluded; host
    gates are the evidence.
11. **Custody plan §9 rows adjacent to this slice (W-6/M6), carried:**
    sequence/journal accounting (3a's journal), cleanup-before-primary ordering
    (context for 3s; row 11 itself is slice 5's), and the
    `bridge-core` 86%→90% / `bridge-workflow` 87%→90% coverage floors — slice 3
    is the largest addition to both crates since the floors were set; each 3a/3b
    fold reports the measured coverage delta, and the floors are claimed by
    slice 6's aggregate closure if still short (M6).

### 1.4 Explicitly NOT slice 3 (named owner; nothing crosses silently)

Slice 4: production clock ARMING (row-10 60 s, preparation 30 s — mechanisms
land here under manual clocks), fixed grace, `AutomaticR2f1b` construction +
`workload_identity()` wiring, watchdog-settings admission refusal, R-6 capacity,
**capability-token construction for the V3 route (M9 — re-adjudicated to slice
4: admission constructs the token with `AutomaticR2f1b`)**. Slice 5: row 11
(wholly — M5), `RecoveredLive` outgoing edges (owner sign-off first), durable
retained identities, `NodeCleanupDispositionV1` cutover, proof-of-removal
token, `predecessor_claim_digest`-is-a-snapshot-digest consumer note (S-5d),
2c2 §4.3 preflight/V3 collision (S-5c — slice 5 finds it armed otherwise),
**the collateral journal's DURABLE half (M7):** `CollateralResultV1` reaches
`NodeCleanupRecordV2.collateral`, whose production writer is slice 5's cutover
— slice 3 lands the flight-side result and its aggregation contract; the
durable row is slice 5's, declared. Slice 6: coverage floors if still short;
final §6-matrix evidence sweep. Trigger-gated, carried NOT activated:
publication-cell blocking on teardown (M14 keeps it untested here, on
purpose); `.custody-locks` flock-GC; descriptor-relative `remove_tree` swap.
The frozen custody table gains no edges in slice 3.

---

## 2. Split — seven sub-slices, strictly sequential folds

```
3s   settlement completeness      executor epilogue (10 exits, settle-before-yield) +
     (FIRST; no flight surface)   epoch linearization + barrier/contention tests
3a   flight core                  journal-backed runner (sync-joinable), OwnedProcessTreeV1
                                  shell, flight-ID cardinality, transfer API + clock seam
3b1  process authority I          Supervised internalization (all 3 paths), AcpBackend
                                  adapter, start-identity probes (darwin+linux), pid gate,
                                  rows 8+9 e2e; diagnostic prerequisite decision record
3b2  process authority II         destructive wrappers join-or-refuse; WorktreeBackend
                                  flight forwarding; THE trait-surface pass (flight API +
                                  cleanup-disposition signature) + doubles ripple, ONCE
3c1  container authority          typed spawn result, ReapController promotion, teardown
                                  entrances enumerated, ContainerRw decorator forwarding
3c2  api authority                request-flight identity, watch-cancellation scoping
3d   preparation + settlement     non-cancellable materialization flight, finite-ownership
     of unused candidates         manual-clock row, UnusedSettled producer, marker authority
```

**Dependency DAG (M1):** 3s ⊥ 3a (disjoint crates/files — 3s: executor +
bridge-worktree epoch internals; 3a: bridge-core flight module) — 3s DISPATCHES
first and FOLDS first. Then strictly: 3a → 3b1 → 3b2 → 3c1 → 3c2 → 3d. No
parallel folds; every dispatch bases on the latest local main; the integration
protocol is the slice-2 one (sequential fold worktree, full gates each fold).
3c1 depends on 3b1's adapter (the composite-boundary claim is unbuildable
before it — M1); Replay's refusal regression lands with 3b2's trait surface
(M17), not 3c.

**§6-matrix ownership (verbatim boundary names — M2/S-2):**

| Boundary §6 / §5.7 row | Named test (boundary's name where it exists) | Owner |
|---|---|---|
| Row 7 admission-closed no-signal | `admission_closed_without_journaled_intent_permits_no_signal` | 3a |
| One-flight / shared generation | `two_nodes_one_generation_signal_once_and_share_result` (needs a real signal → 3b1; 3a proves the keying half only) | 3b1 |
| Row 8 join-if-live, never PID/name | mechanism 3a; end-to-end through AcpBackend 3b1 | 3a+3b1 |
| Row 9 SIGTERM-ignored escalation | `shared_acp_escalation_lists_every_active_owner` | 3b1 |
| Unrelated-process survival (slice gate) | `unrelated_process_with_recycled_pid_survives_every_flight_action` (via injectable authority ports, B10; host integration secondary) | 3b1 |
| Row 10 transfer mechanism | `cleanup_deadline_transfers_exact_guard_before_terminal` + failed-transfer→unknown negative — manual `MonotonicClock`, transfer API in 3a, driven test in 3a; slice 4 arms production | 3a |
| Preparation finitely owned | `nonreturning_custody_sync_transfers_pre_effect_owner` — manual `PreparationClockV1` | 3d |
| Row 3 marker half | `unused_candidate_settles_only_after_exact_absence` | 3d |
| Row 11 | slice 5 (wholly, declared — M5) | — |

**Sizing (re-based on the measured record — B30/W-3).** Slice-2 landed at
median **3.2×** its brief's estimate tops (2b2 hit 4.6×). Estimates below are
EXPECTED LANDING totals (already ×3 from mandate surface); the STOP-and-report
checkpoint is **1.5× the stated landing estimate** — a genuine tripwire, not a
routine crossing: 3s ~1,800; 3a ~2,500; 3b1 ~3,000; 3b2 ~3,000; 3c1 ~2,200;
3c2 ~1,500; 3d ~2,500 (totals incl. tests).

**Lens: dual for all seven** — 3b1/3b2/3c1/3c2 mint signal authority (the S4
destructive-surface rule); 3d licenses marker removal; 3a defines the
admission/journal semantics five destructive sub-slices inherit, so a defect
there is inherited five times (S-8's corrected rationale); 3s for blast radius
— it touches every exit of the executor's generator, exactly where both 2c2
lenses found their shared defect outside the diff (m3's corrected rationale).
One-round cap + one targeted repair each.

**Pipeline (standing):** bridge implement (terra/sol high/xhigh, the COMMITTED
`examples/a2a-bridge.r2f1b-impl.toml`, `--lang rust`, `--depth light`);
operator boundary on every hand-off (inspect the diff; verify internal-review
objections against source — they have been wrong in both directions); dual-lens
review (opus senior-lead + sol via `run-workflow code-review`); implementer
handoffs mirrored under `docs/superpowers/reviews/` and a dual-review record
per sub-slice committed with each fold (m4); **fold ritual per sub-slice:**
squash to local main in the fold worktree, then the EXECUTABLE gate block (M19)
— `git diff --check && cargo fmt --all -- --check && cargo clippy --workspace
--all-targets -- -D warnings && cargo test -q --workspace && cargo build
--release --bin a2a-bridge && ./target/release/a2a-bridge validate
--repo-hygiene`, per-stage exit codes, exact totals recorded in the dual-review
record + handoff ledger (m2), push, roadmap-cursor update (M21), **and reap the
sub-slice worktree's cargo target after its branch folds** (the 2026-08-09
143 GiB lesson — receipts in the session scratchpad). Host-only process tests:
the flock/exec/descendant families are hermetic-container-excluded; host gates
are the evidence and every gate report says so.

---

## 3. Per sub-slice

### 3s — Settlement completeness (dispatch FIRST)

**Scope.** (a) The exactly-once settlement epilogue: capture the primary
error, run per-checkout settlement (`NotHealthy` with the correct
`NodeFailure`/`Cancellation` mapping) for every materialized checkout, THEN
yield the terminal `Err` (B23) — covering the 10 post-recording exits of
`run_from_with_context_inner` (§1.2 list; re-enumerate at dispatch); the 11
pre-scheduling exits stay effect-free, pinned per exit FAMILY
(harvest/policy-finalize/encode/invariant — B24), not by one representative.
(b) Epoch linearization per §1.3.6 (declared default: linearize; exception
branch needs the written mechanism proof) + sol S-1's deterministic barrier
tests (preserve-raised-between-check-and-CAS, both orders) + the
writer-vs-settlement contention schedule in both orders with expected custody
state named (M10). (c) Re-verify `authorize_deletion`'s no-record-ownership
basis under the new concurrency (§1.3.9). **Anchors:** `executor.rs:4628`
(generator), `:5455-5476` (current pass), `:2056`/`:3499` (recording);
`backend.rs:1300`/`:1819` (generation check), mint in `custody_writer.rs`.

**Red first:** consumer-drops-after-error regression; one red per post-recording
exit family (the `FailingHarvestAuditStore` shape generalized); pre-recording
exit settles nothing; both barrier orders; both contention orders. **Ripple:**
none on the trait surface (uses the landed `settle_workflow_checkout_v1`).
**Non-goals:** no flight surface, no signal changes, no trait changes.
**Design note owed:** where the epilogue lives in the generator and why the
before-yield ordering cannot be starved.

### 3a — Flight core

**Scope.** The `RetainedResourceFlight` runner over the landed
`ResourceFlightStateV1`: admission close under one transition lock; owner
snapshot in deterministic order; **journal-before-dispatch on a DURABLE
substrate** (B7 — the record type, its storage port, capacity contract, atomic
transition API, and recovery lookup are the implementer's design, but the
CONSTRAINTS are fixed: durable before any signal dispatch; bounded capacity
refuses admission before provider work; crash/reopen recovers the journaled
intent; **joinable from a non-async context** — `terminate_blocking` runs on a
bare `std::thread`, B13); **flight-ID cardinality defined and persisted** (B8:
node-owner ↔ generation-flight mapping for shared processes, per-request
flights for API; its reservation CAS and `NodeCleanupV2` aggregation contract);
**attach-vs-discover semantics** (M12: transition-locked `attach_owner` refuses
after close; `discover_collateral_owner` records collateral discovery — two
different things, both journaled, capacity-bounded, race-tested); the **row-10
transfer API** (B22: transfer of the exact retained guard + `RecoveryOwnerV1`
publication lives HERE on the runner, driven by the injected `MonotonicClock`
under a manual clock — `cleanup_deadline_transfers_exact_guard_before_terminal`
+ its failed-transfer→unknown negative); `OwnedProcessTreeV1` as the capability
SHELL (identity capture + journal hooks, NO signal wiring). **Anchors:**
`resource_flight.rs` (types, `:80-84` start identity), `attempt_activity.rs:142`
(the one injected clock — no parallel clock type, boundary §4.1).

**Red first:** row 7; row-8 mechanism (join the SAME flight object; dead flight
+ journaled intent → `Unknown`, never PID reconstruction); capacity refusal;
snapshot order; late-discovery delivery; one-flight keying half; crash/reopen;
row-10 transfer + negative. **Non-goals:** no backend adapter, no real signal,
no executor touch. **Design notes owed:** journal substrate + capacity
contract; cardinality mapping; sync-join mechanism.

### 3b1 — Process authority I (the load-bearing destructive surface)

**Prerequisite (M18):** the bounded descendant-kill diagnostic — hypotheses,
same-environment control, decision record (absorbed vs re-parked with owner +
hooks) BEFORE implementation.

**Scope.** `Supervised` construction internal to `OwnedProcessTreeV1`; **all
three signal paths** join-or-refuse: `Drop` `:530-546` (unconditional group
SIGKILL — the exact §5.4-forbidden shape), `terminate` `:403-422`,
`terminate_blocking` `:428-475` (B1). **Flight-before-spawn** (B11): the flight
exists and is journal-capable BEFORE the process spawns; the returned process
binds under the transition lock; partial spawn/bind failure records
protectively. **Start-identity probes on BOTH platforms** (B3): darwin
(`sysctl KERN_PROC_PID` → `p_starttime`) and Linux (`/proc/<pid>/stat` field
22), with a stated wire decision if tick semantics differ; the Linux lane is
evidence-or-declared-exclusion in §6. **Child-first SIGKILL mechanism** (B9):
a descendant registry with per-child immutable identity and a deterministic
fake-OS ordering test — a bare group signal cannot order children. AcpBackend
adapter: the flight owns `Supervised`, the group, start evidence, the optional
`:ro` controller; `cancel`/`escalate_terminate` (`:5160`)/`retire`/registry
retirement/`Drop` all join; ordinary release detaches one owner. **Injectable
process-authority port** (B10) so the pid gate is deterministic (same numeric
pid, different immutable identity → refused); bounded host integration
secondary. **Every direct `Supervised::spawn*` caller migrated** (B6: prod
`acp_backend.rs:3119` + bridge-acp test helpers + three bin integration tests).
**V2 decision + controls (B17):** flights attach universally at the process
layer (boundary §3: direct sessions must attach an owner if they can share a
process) but destructive REFUSAL semantics are V3-gated in this slice — V2
teardown behavior byte-identical, pinned by explicit V2 lifecycle controls per
changed path (including the `Drop` leak-backstop question: a V2 session's drop
must still reap exactly as today).

**Red first:** the pid-survival gate (port-driven); rows 8/9 end-to-end
(`shared_acp_escalation_lists_every_active_owner`;
`two_nodes_one_generation_signal_once_and_share_result`); drop-outside-flight
refusal (V3) + V2 drop control; child-first ordering (fake OS); flight-before-
spawn crash windows. **Ripple:** spawn-caller migration only — the trait
surface waits for 3b2.

### 3b2 — Process authority II (wrappers + THE trait pass)

**Scope.** The ONE `AgentBackend` surface change (S-3/B5/B25): the flight
attachment/exposure API (names/signatures = implementer design note; refusing
defaults; production overrides enumerated) AND the typed cleanup-disposition
signature (R-5: the session-manager finally sees
retained/preserved/unknown — public type decision is the design note; red
tests that each disposition reaches the session-manager without collapsing to
`Complete`, M11) — swept over the ~120-impl population ONCE, enumerated from
source at dispatch (B12). Destructive wrappers join-or-refuse with one red
each: registry drain/race-loss/invalidation/reload/keyed retirement
(`registry.rs:533`), `ResilientWarm` (`resilient.rs:178-192` —
retire→reset→rebuild UNREACHABLE for protected attempts), session-manager
cleanup, coordinator dispatch cleanup. **`WorktreeBackend` flight forwarding**
(B2/W-2): the decorator forwards the inner flight (positive end-to-end test:
a protected-V3 teardown reaches the inner flight; an unforwarded default
CANNOT signal); composition-basis restatement in the handoff (§1.3.8) +
wrapper-composition disposition tests (M15: inner/outer protective
`Retained`/`Preserved`/`Unknown` neither collapsed nor double-signaled).
Replay refusal regression pinned here (M17). V2 controls per changed wrapper
(B17).

### 3c1 — Container authority

**Scope.** **Typed spawn result** (B14): `ContainerSpawn` currently returns
only `Arc<dyn AgentBackend>` and `ReapController` arms pre-spawn on
`(runtime, name)` — the seam cannot carry the immutable container ID; define
the typed result / pre-ID state (cancellation in the pre-ID window refuses and
retains `Unknown`), then promote `ReapController` (`reaper.rs:78`) INTO the
flight; no name-based removal remains. **`ownership_labels_digest`
canonicalized** (M16): one shared constructor/validator, label order pinned,
goldens + extra-label negatives. **Teardown entrances enumerated from source
at dispatch** (B16): spawn/configure/prompt failures, cancel, checked/observed
forget+release, retire, stream teardown, `Drop` — one join-or-refuse red each.
ContainerRw decorator forwarding half (B2's second site; 2c2 census
precedent). Composite boundary: the inner ACP process subordinate to ONE
flight (buildable now that 3b1 landed the adapter — M1). Recycled-container-
name gate via the injectable runtime port (B10). V2 controls (B17).
**Anchors:** `bridge-container/src/lib.rs:1298`, `reaper.rs:78`.

### 3c2 — API authority

**Scope.** **Request identity defined** (B15): one prompt may issue multiple
sequential POSTs/tool rounds and no request identity exists — specify when
each `DedicatedRemoteRequest` flight is minted/journaled/attached/settled, how
the watch sender is scoped so a stale flight CANNOT cancel a successor round
(the two-round negative is the load-bearing red), and node-level aggregation.
**Anchor:** `bridge-api/src/backend.rs:644`.

### 3d — Preparation flight + candidate settlement (recovery side)

**Scope.** (a) The claimed, non-cancellable materialization flight — first
production writer of `PreparationFlightStateV1`. **Terminal-state check FIRST**
(B21): the landed type has `Open`/`BarrierSynced`/`Transferred`/`Failed` and NO
success-settlement state — the dispatch brief resolves whether `Transferred`
is the success terminal or the wire type needs amendment (goldens +
serialization tests + exhaustive matches in the SAME change if so; A2's
goldens are the tripwire). The runner retains the map/provider/custodian
`Arc`s across caller-future drop; **phase-distinguished cancellation tests**
(M13): before-claim / after-claim-before-add / mid-add / after-add-before-
evidence / terminal-publication-failure, each with its expected durable state.
(b) **Finite ownership** (M3): `nonreturning_custody_sync_transfers_pre_effect_owner`
under a manual `PreparationClockV1`; slice 4 arms production. (c) **Candidate
settlement (owner ruling):** the recovery-side `UnusedSettled` producer —
**executable seam specified as a constraint** (B18): the sweep is sync and the
registration probe is async+private in `host_git`; the implementer designs the
async/trait recovery seam (provider recovery query or equivalent), boot-caller
wiring, and tri-state refusal (present / absent / cannot-prove → refuse), as a
design note the review checks. **The settlement holds a refusing lock window
across proof→transition→unlink** (B19: a concurrent materializer must not
invalidate the proof mid-operation; both-order contention tests; does NOT
activate the parked blocking-acquisition policy). **Descriptor-safe removal**
(B20): same-object descriptor-relative transition-then-unlink, no-follow,
parent-synced, crash-ordering + replacement/symlink negatives. (d) **The 2b2
marker population** (§1.3.2): the marker-removal authority keyed on
state-agnostic exact-absence proof serves BOTH populations; no table edge.
**Anchors:** `backend.rs:2205` (`materialize_under_custody`), `custody.rs:393`
(the one entry edge), `preparation_flight.rs:127` (`PreparationClockV1`),
`sweep.rs` (both arms).

**Red first:** `unused_candidate_settles_only_after_exact_absence`
(present-target refuses; registered-but-absent refuses; both-absent settles,
marker only); dropped-configure-future per phase; the finite-ownership row;
contention both orders; replacement/symlink negatives.

---

## 4. Risks and disposition

| # | Risk | Trigger / likelihood | Disposition |
|---|---|---|---|
| R3-1 | Trait-surface ripple (~120 impls) | 3b2; certain | ONE consolidated pass (S-3); population enumerated from source at dispatch |
| R3-2 | Parked kill-escape flake contaminates 3b1 evidence | likely during suites | Diagnostic prerequisite + same-environment controls; family documented (2c2 record); hermetic exclusions named in every gate report |
| R3-3 | Journal write on hot paths | every signal | Bounded capacity + refusal-before-work is designed behavior; observed (not gated) at 3b1 fold — a non-blocking observation, no benchmark contract (m1) |
| R3-4 | Platform start-identity divergence | darwin vs linux; certain | 3b1 work item + wire decision + evidence-or-exclusion in §6 (B3) |
| R3-5 | 3s destabilizes V2 executor paths | epilogue touches every exit | Strict addition on exits that settled nothing; V2 controls + consumer-drop regression (B23) |
| R3-6 | Sizing blows one-round review | measured slice-2 median 3.2× | Estimates ARE landing sizes; STOP at 1.5×; seven smaller sub-slices (B30) |
| R3-7 | 3d terminal-state ambiguity forces a wire change mid-round | `PreparationFlightStateV1` has no success state | Resolved at 3d dispatch-brief time, BEFORE implementation (B21) |

## 5. Rulings and standing constraints

Two-phase settlement ratified-as-shipped: 3s's epilogue keeps per-checkout
independence — no global recompute-after-teardown. The 2b2 transition ruling
stands: recovery-side producer only; the frozen table gains no edges (the
marker authority is a removal API, not a transition). 2c1 RE-3: flights are
OWNERS, never outcome authorities — context-free callers still never arm
`Preserve`. **PARK-vs-acceptance (B29):** a red MANDATED acceptance test blocks
the fold and gets the bounded repair; PARK is reserved for pre-existing /
out-of-scope defects discovered along the way (own PR, named owner). Standing
rule otherwise unchanged. Bridge-implement operator boundary on every hand-off.

## 6. Exit gate (slice-4 entry conditions)

1. Every §2 ownership-table test green by name on host (rows 10-mechanism and
   preparation-finiteness under manual clocks; row 11 and production arming are
   the DECLARED remainders to slices 5 and 4 — the gate certifies mechanisms,
   not the absent halves, B28).
2. `unrelated_process_with_recycled_pid_survives_every_flight_action` (port-
   driven) + `two_nodes_one_generation_signal_once_and_share_result` green;
   Linux start-identity lane green in CI or its exclusion declared with the
   darwin evidence named (B3).
3. `unused_candidate_settles_only_after_exact_absence` green; both marker
   populations served (owner rulings discharged).
4. **Scoped raw-signal census (B27):** within the R2f1b-reachable set — the
   three `Supervised` paths, the five backend adapters, registry/resilient/
   session-manager/coordinator wrappers, `ReapController`, container teardown
   entrances, API watch cancellation — every signal/retire path journal-flighted
   or refusing, verified by the sub-slice §2c self-passes plus one aggregate
   census at 3d's fold. EXCLUDED with reasons, named in the census: the
   compatibility harness's own process groups (test infrastructure), doctor/
   attested-wrapper children (diagnostic subprocesses, no R2f1b authority),
   worktree Git subprocesses (bounded commands, not owned resources), and any
   `kill_on_drop` outside the R2f1b-reachable set — each listed, none silently.
5. Full workspace gates at every fold (the §2 executable block), exact totals
   with per-stage exit codes, recorded in the sub-slice dual-review record and
   the handoff ledger; roadmap cursor updated per fold; review record +
   implementer handoff committed per sub-slice (m4).
6. Coverage delta for `bridge-core`/`bridge-workflow` reported at 3a/3b2/3s
   folds against the §9 floors (86%→90% / 87%→90%); floors claimed by slice 6
   if still short.
