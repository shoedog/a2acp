# Slice 2b2 V3 routing + writer + creation ordering — dual-lens review record

Date: 2026-08-09. Artifact: `feat/r2f1b-2b2-routing-writer` @ `3a655fc6` → repaired
`36524ea6`/`28499e25` (base `8255cf5f`). Opus senior-lead: REVISE — 4 WRONG (all DEFER-
classed individually, W-1 named repair-worthy), 12 SMELL; S7 lock ordering PASS
(deadlock-freedom argued case-by-case), §2c call-site property HOLDS with the count
corrected (THREE production admission sites, not four). Sol: REJECT — 5 BLOCKERs + 2
deferred WRONGs. Heavy overlap between lenses from different angles; every finding
closed-enumerable; both endorsed targeted repair over restart. Declared cap held: one
round, one repair, no second review round; repairs verified by mutation evidence on the
branch and the aggregate fold gate.

## The adjudicated repair (R1–R9, all landed in `36524ea6`)

1. **Routing handoff (sol B-1 / opus S-6).** `AdmittedWorkflowRunV1.r2f1b` was write-only
   in production — both production authority consumers dropped it, so a `Some(manual)`
   admission would have silently degraded to V2 (legacy add + `.meta.json`) on
   activation, with no failing test. Repair: `with_admitted_workflow_run` destructures
   the admitted run (spec → optional contract) at both consumers; end-to-end red test
   drives the REAL freeze → binder → backend path with a V2 negative. Production
   unreachability is unchanged (all three production admission sites pass `None`).
2. **Sweep-side publication cell (sol B-2, + opus S-8).** Boot and run-end sweeps
   deleted outside the S7 cell, contradicting `custody_lock.rs`'s own caller contract —
   a sweep racing a writer could delete a just-protected checkout. Repair: forgery
   guards first (fixing the probe-before-guards ordering on forged sidecar paths), then
   the refusing cell held across probe + all removals; raced both arms, both orders.
3. **Residue identity safety (sol B-3 / opus W-4a, S-4, S-10).** Two pathname unlinks in
   the writer's failure/durable arms could delete a same-name foreign file (the durable
   arm's unlink was ONLY ever destructive — if the name is free it is a no-op). Repair:
   both removed; residue-kept policy now actually tested (the two tests the obligation
   table had cited did not assert it); staging recognition tightened to exactly 32
   lowercase hex; reclamation owner stated truthfully (storage-report visibility +
   owner disposition — the boot sweep ignores staging names by design).
4. **False permanent `Materializing` (sol B-4 / opus S-3).** Provider `Err` (including
   the refusing default) left a durable `Materializing` record with no claim/locator.
   Repair: `supports_custody_add` preflight before any record effect; post-preparation
   runtime `Err` settles `PreservationUnknown{materialization_inflight}`.
5. **Settlement epilogue (sol B-5 / opus W-2).** Handled terminal arms (implement
   abort/no-commit; run-workflow early error return) bypassed `settle()`, and the guard
   logged `settled = !unwinding` (true on unsettled clean drops). Repair: one settlement
   epilogue per guard site; log field = `is_settled()`; a non-panicking Drop now marks
   settled, making "unsettled" mean exactly "panicked or unhandled" (the `phase` field
   still separates explicit settle from clean drop).
6. **Vanished-root arm made real (opus W-1a).** The gate cell's lock acquisition ran
   `create_dir_all` on a teardown path — re-creating a vanished `[worktrees].root`; the
   documented `Ok(None)` arm keyed on an error `create_dir_all` prevents. Repair:
   `try_exists()` precheck before the cell.
7. **Claim honesty (opus W-3):** add-failure `common_dir` identity records the
   plan-derived `<source>/.git`, not the source repo path.
8. **Small mandated set:** storage-report coexistence via the pre-existing
   `merge_holder` (Held dominates; the existing lattice's Unknown-beats-Free rule
   preserved — the reviewer-drafted version would have regressed it); lock-order
   re-declaration including the publication cell + the file-lock-inside-`cell.state`
   nesting note (opus S-1); `bind_custody_plan` doc corrected (ordinal-0-only
   re-verification, opus S-5); reverse-order cell race + 5 cell unit tests (opus S-2);
   staging-name negatives (sol W-7).
9. **Naming/handoff:** the deviation test renamed to
   `add_failure_before_any_target_preserves_unknown_and_touches_nothing`; §5.7 row-1
   mapping fixed; four→three production-site count corrected.

## Transition-table ruling (owner escalation resolved at review level)

Both lenses: the shipped protective retention is CORRECT; **do not add
`Materializing → UnusedSettled`.** Opus's mechanism-level reading, adopted: §5.7 row 3
("prepared synced, before `git add`") is a CRASH recovered from `ProtectionPrepared` —
the state the frozen edge already serves — so `UnusedSettled` is a recovery-side
transition, not an in-line writer transition; the add-failure path routinely cannot meet
§2.2's descriptor-bound proof precondition (that is what `RegistrationUnproven` exists
for); and 2a's `identity_completeness()` data anticipated exactly this arm
(`PreservationUnknown{MaterializationInFlight}` is the only degraded-legal reason,
citing §5.1). The brief's mandated test NAME was the mis-specification; renamed.

## Ledger

- **OWNER ACCEPTANCE NEEDED (2 items):**
  1. `.custody-locks` lock-file residue — one small file per checkout per run, never
     unlinked BY DESIGN (F-3: `PersistentLockGuard::drop` must not unlink). Accept the
     residue (storage-report classifies it), or commission a reclamation design (flock
     unlink safety analysis required — do not improvise).
  2. Pre-target add failures retain a protective `PreservationUnknown` marker until a
     later authority handles them (per the ruling above). If accumulation ever becomes
     unacceptable, the remedy is a narrowly-scoped marker-removal authority
     (`target == ProvablyAbsent AND locator == UnregisteredDirectory`) in the slice
     owning marker removal — NOT the table edge.
- **Re-anchored (opus S-11):** 2b1's sol-1 deferral rested on "`Protected` is
  cfg(test)-only AND no production path writes custody records." The first clause is
  now false (the writer sets it); the deferral survives on production-unreachability of
  the writer alone (three `None` admission sites + both-entrance `AutomaticR2f1b`
  refusal + no production `Some`-minting path). 2c1 still owes the typed
  retained/refused disposition before refusal becomes production-reachable.
- **2c1:** success-path identities (source/root/common_dir) are captured then discarded
  — `LiveProtected` forbids a claim; retain them in protected backend state and
  reverify at claim-mint (opus S-9 / sol S-3). Settlement of the publication cell,
  typed retained disposition, Reserving ownership retention (carried).
- **Slice 3/5:** capability-token construction for the V3 route (stronger than the
  call-site `None` property — opus: rightly out of 2b2's scope); `UnusedSettled` and
  its `ProtectionPrepared → UnusedSettled` edge currently have NO producer — assign the
  recovery-side owner or it is dead wire contract.
- **Trigger-gated:** `spawn_blocking` for gate probe + cell acquisition (second, heavier
  caller now — opus S-7); Linux-arm/real-NFS execution; `supports_custody_add` ↔
  `add_under_custody` two-method coupling (both default refusing; stronger tie if a
  provider ever claims support falsely).
- **Posture note:** opus's honest reviewability assessment — the diff was slightly past
  one-round reviewable, and its costliest find (W-1) lived two call frames OUTSIDE the
  diff in unchanged code. Standing lesson adopted: a slice that wires a new lock into an
  existing destructive funnel carries the funnel's transitive resource behavior in its
  own review scope.
