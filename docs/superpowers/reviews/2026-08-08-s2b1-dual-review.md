# Slice 2b1 protection primitives + deletion gate — dual-lens review record

Date: 2026-08-08/09. Artifact: `feat/r2f1b-2b1-protection-gate` @ `fb9aad76` → repaired
`775db6b3` (base `b4fc1ff3`). Opus senior-lead: SHIP — 1 WRONG (a false coverage claim in
the handoff document, not the code), 10 SMELLs all DEFER, all three implementor gate
decisions ENDORSED after mechanism verification (the failed-configure retry-spin rationale
for Ok-on-refusal is real: 13 rollback sites set the pending flag and the loop's only exits
are an Ok report or a flight handoff; no consumer anywhere infers "checkout gone" from a
cleanup Ok). Sol: REJECT, 3 WRONG/BLOCKERs. Declared cap: one round, one targeted repair,
no second sol round.

## Adjudication (orchestrator, on primary evidence)

- **Sol-3 (replace `Err` ≠ provably-not-renamed under NFS error-after-effect) — CONFIRMED,
  REPAIRED.** `replace_regular_child_impl` mapped the `renameat` errno straight to `Err`
  while the contract claimed nothing had moved. Repair makes the claim true instead of
  weakening it: `classify_failed_replace_rename` decides on descriptor identity in evidence
  order (source present + identity-match → true `Err`; target identity-match → committed,
  post-commit path continues with the errno carried in `Durable{retried_rename}`;
  otherwise `RenameOutcomeUnverified`, protective). Positive-match discipline: a same-name
  substitution never reads as proof of no effect. Red-first via the new
  `ReplaceRenameFaultV1` seam (a fault seam must state what the filesystem DID, not just
  the errno); mutation-checked in both directions.
- **Sol-1 (refusal projected as Complete disposition) — DEFERRED to 2c1.** Mechanism-level:
  `WtCustodyV1::Protected` is cfg(test)-only and no production path writes custody records
  (both lenses swept), so the only production-reachable trigger is a transient inconclusive
  probe, whose consequence is a protective, self-healing leak (run-end guard / boot sweep).
  The fix is the typed retained/refused cleanup disposition — 2c1's vocabulary by design.
  2c1 gate obligation: once refusal is production-reachable, a refused checkout must never
  project `Complete`.
- **Sol-2 (refused `Reserving` rollback loses its last cleanup owner) — DEFERRED, split.**
  Real and confirmed in source (`entry_for_cleanup` pops the reservation; the reporter
  evicts the cell on Ok). A safe in-slice fix would invent retained-state semantics 2c1
  owns; re-inserting as `Ready` collides with Ready-means-reusable (opus S-3). 2c1 owns
  ownership-retention-through-refusal; 2b2 inherits the sharpened R-7 (below).
- **Opus W-1 — CONFIRMED, REPAIRED (docs).** The handoff claimed 2a's
  `legacy_boot_arm_still_reclaims_alongside_a_v3_record` pins the one-checkout-carrying-
  both-records case; that test builds two separate checkouts. The coexistence state is
  exactly what the gate produces on refusal (sidecar retained beside the record), and the
  legacy arm — including the run-end guard's CLEAN-DROP arm, every normal run, no crash
  needed — would delete such a checkout. Rewritten as an open two-half 2b2 obligation:
  writer-never-emits-`.meta.json` is insufficient because the gate itself manufactures
  coexistence.

## The round's second find (implementor, during repair)

**PARKED-1: `rename_child_no_replace` carries the identical errno-trust hazard in the
FAIL-OPEN direction** (merged A4 code — parked per custody plan §4; own bounded PR).
*FIXED 2026-08-09 — see `2026-08-09-noreplace-classify-review.md`: shared identity
classification for both primitives (`CustodyPublicationV1` replaces
`ReplacePublicationV1`), a second pre-existing fail-open route closed (publish returning
`Err` after a successful rename on parent-sync/identity-recheck failure), and the
`local_file` journal-publication path classified with its caller invariants verified.* A
retried NFS RENAME can create the target and report ENOENT: a `ProtectionPrepared`
publication then tells the writer the checkout is unprotected while the record exists.
2b1's disk arm contains the deletion consequence (presence-keyed refusal), but writer
control flow is still wrong. Fix is mechanical (`classify_failed_*` shape, rule 2 =
"target exists AND is our object ⇒ committed"); cross-reference left in
`rename_child_replacing`'s docs.

## Ledger

- **2b2 (binding):** add-prohibition (custody-aware add) lands in the SAME PR as the
  writer — the gate's own refusal makes the `cleanup_failed_add` path routine (refused
  rollback → configure retry → `git worktree add` fails on the surviving dir →
  `remove_dir_all`); both-records coexistence test
  (`a_checkout_carrying_both_records_is_reclaimed_by_neither_sweep_arm`, both sweep arms
  incl. run-end clean drop); sweep redundant-guard coverage item (carried from 2a);
  storage-report classification for `<root>/.custody-locks` (opus S-10, report noise only).
- **2c1 (binding):** typed retained/refused cleanup disposition + truthful projection;
  ownership retention through refusal; explicit reuse policy for `Ready` entries whose
  checkout is custody-preserved (opus S-3); hold the refusing custody lock across
  probe→removal→settlement (sol SMELL-1); `before_rename` injection seam for the replace
  path (opus S-8); `#[must_use]`-proof ambiguity handling (opus S-7).
- **Trigger-gated:** `spawn_blocking` for the gate's probe if a networked worktree root
  appears (opus S-6); Linux-arm CI execution for the custody primitives (sol SMELL-2).
- **Erratum applied to the slice-2 brief:** risk R-2 "RESOLVED BY 2b1" downgraded to
  partially resolved — the gate covers the R-11 fan-in; `sweep::remove_worktree` and
  `host_git::cleanup_failed_add` remain ungated until 2b2 (opus S-1).
- **Accepted, named:** every V2 cleanup now probes disk; a transient root-read failure
  refuses an ordinary V2 removal (protective leak, self-healing) — the correct fail-closed
  trade, now documented (opus S-5). Upward test-only dev-deps `bridge-worktree` →
  `bridge-coordinator`/`bridge-controller`: structure ENDORSED by both lenses (cycle-free,
  no feature unification side effects); the handoff's "only option" justification refuted
  by opus, real cost named (focused worktree tests now compile both crates first).

## §2c note

The implementor's SELF-PASS refuted its own brief's broad funnel claim (three production
removal sites, not one) and the narrowed R-11 claim survived independent opus
verification, which also closed the enumeration with the one site the sweep missed
(`compatibility_resolution.rs` — not a checkout). Both lenses converged on the same
mechanics at every contested site; the verdict split was severity interpretation only,
resolved here by reachability + repair-ownership analysis.
