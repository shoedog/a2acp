# Slice 2c1 fail-closed preservation — dual-lens review record

Date: 2026-08-09. Artifact: `feat/r2f1b-2c1-preservation` @ `884ab1f3` → repaired
`b5c7f1ba` (base `a9962e25`). Opus senior-lead: REVISE — 2 WRONG/BLOCKER + 1 WRONG/DEFER
+ 10 SMELL; verified the preservation-only invariant HOLDS (independent token sweep +
call-graph read), the P5 custody-positive split CORRECT in both directions, the P3
epoch join key correct-as-a-key, and audited all five binding ledger items. Sol: REJECT
— 7 BLOCKERs + 2 SMELLs. Declared cap held: one round, one repair, no second review.

## Adjudication highlights

- **Repaired (both lenses):** the `PreservationPrepared` strand (the frozen table gives
  it two legal exits; the driver refused both — the exact crash-between-renames case the
  two-step exists for; resume arm re-derives `verified` from live objects, never
  laundering the stranded claim) and the locator no-downgrade (a durable claim could
  assert `RegisteredWorktree` after reverification disproved it; widened to the prepared
  publication's crash window).
- **Repaired (sol, adjudicated in-scope):** the cold non-success inventory never armed
  `Preserve` — 13 sites converted + 2 computed, centralized in
  `ColdExitV1::preservation_reason()` with `Success => None` structural; severity
  corrected during adjudication (the gate + `Retained` reuse-refusal already contain the
  overwrite vector sol claimed — the real cost was recovery-evidence quality, claims
  never minted). And the materialization window: protected evidence now lands on the
  reservation immediately after `materialize_under_custody`, before any await — an
  inner-configure failure can no longer leave a `Legacy` entry beside a `LiveProtected`
  record; residual future-drop window documented; sol's claimed-noncancellable-flight
  design LEDGERED to slice 3 (preparation-flight runners).
- **REFUTED (recorded as a ruling on `preserve_checkout_v1`):** sol's "preserve before
  every manager-owned cancel." Context-free callers (SessionManager, drops, reaper)
  must NOT arm `Preserve` — they have no outcome context by design (the R-11 property
  the single-gate architecture rests on), and unconditional manager-side preservation
  would terminalize healthy idle warm sessions. Context-free deaths gate-retain; the
  workflow-outcome owner (2c2) disposes.
- **Deferred, standing:** sol B-5 (transient-refusal `Reserving` orphan) = the 2b1
  accepted self-healing trade, opus concurring — now explicit in the handoff §4.4; sol
  B-6 (`Complete` persisted for a retained checkout) = the V2-inconclusive half was
  owner-accepted at the 2b1 fold, the V3 half is the declared slice-5 remainder per the
  §5.3 cutover ruling — boundary sentence corrected per opus (only the DIAGNOSTICS
  transition code distinguishes; the durable node-terminal row does not, until slice 5).

## Ledger

- **2c2 (binding):** disposition monotonicity across cell eviction (opus W3 — the
  `Preserve` epoch dies with the cell on the first `Ok` report; a later context-free
  flight reports `Retained` for a checkout whose durable record says `Preserved`;
  label-only today, but 2c2's workflow-level disposition owner must make the invariant
  durable); post-loop mint + disposition of gate-retained context-free deaths.
- **Slice 3:** claimed, non-cancellable materialization flight (closes the documented
  future-drop residual window); session-manager disposition bookkeeping (R-5 signature
  churn).
- **Slice 5 (restated hard):** the typed retained/preserved disposition must reach the
  durable node-terminal row at the V2→V3 cutover — until then a retained checkout
  persists `NodeCleanupDispositionV1::Complete` (accepted; diagnostics code is truthful).
- **Trigger-gated:** blocking publication-cell acquisition on the teardown path is a
  new unbounded wait on shutdown (opus S1 — no cycle, but no-cycle ≠ no-stall; revisit
  if a cross-process target-path collision is ever observed); deterministic barriers
  for the P3 race tests (sol S-1); composition invariant for the defaulted
  `preserve_checkout_v1` if backend nesting ever changes (sol S-2).
- **R2f2 inputs:** off-unix, `identities_reverify` is always false → every preservation
  settles `PreservationUnknown{AmbiguousCleanup}` (restated platform exclusion);
  `AmbiguousCleanup` reason-loss on failed reverification (a 7th `PreservationReasonV1`
  would amend 2a's frozen wire contract — deliberately not done); §5.1 step 5 (lease
  transfer) is 2d's.
- **Posture note:** both opus BLOCKERs were found by reading code against the handoff's
  own prose — the handoff claimed behaviors (locator downgrade, ambiguous-then-
  retriable) the code did not deliver. Same class as 2b1's W-1. Standing lesson: an
  implementer handoff's behavioral claims are review surface, not context.
