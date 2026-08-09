# Slice 2a custody reader — dual-lens review record

Date: 2026-08-08. Artifact: `feat/r2f1b-2a-custody-reader` @ `20b784eb` → repaired `352f9133`
(base `fc98e343`). Opus senior-lead: SHIP, 1 non-blocking WRONG + 5 SMELLs, all six implementor
pushbacks ENDORSED (incl. RecoveryLocatorV1's authored 3-variant set — exhaustive over host_git's
Result by construction; 2b2 must map the Err arm to RegistrationUnproven or the variant is
unreachable). Sol: REJECT, 3 contract-shape blockers, adjudicated to bounded repairs (Option
identity fields mean the pre-identity encoding existed; the missing piece was the per-state rule).

## The round's find (Opus WRONG-1) and repair

R9's deferred-to-boot reclaim was unreachable on clean exit (LeaseGuard::drop unlinks the lease;
probe reads None; classify yields Unknown never Dead) — every clean [worktrees] run leaked
permanently, masked by a crash-shaped test probe. Repair: legacy run-end reclaim gated on
!thread::panicking() (deletion authority removed from exactly the unwind path — R9's stated
thrust); V3 arm unconditionally non-destructive; the mechanism pinned with the REAL lease + probe
so it cannot be simplified back.

## Repairs P2-P6 (all red-first at 352f9133)

Per-state claim/identity rule settled AS DATA (ClaimPresenceV1 x IdentityCompletenessV1; degraded
claims legal exactly where no live identity exists; partial dev-xor-ino refused; unix-only
completeness enforcement named as the platform exclusion). Sweep verifies recorded dev/ino by
descriptor against the sibling; mismatch => Recover. Decoder cross-checks execution-id/attempt
coherence + the ordinal-0 origin rule (resumed-record positive control) and the reason duplication
kept deliberately (both copies load-bearing; documented). OverBound read-layer fixture added.

## Implementor pushback upheld with mutation evidence

The "vacuous" sibling-mismatch test regained discrimination when P1 restored the clean-exit path —
NOT renamed. The investigation exposed a pre-existing redundant-guard gap (sidecar_file_matches and
worktree_under_root each individually neuterable with both tests staying green); recorded in the
docstring as a 2b coverage item rather than papered over.

## Frozen-doc errata (Opus ruling, applied with this fold)

Focused boundary §2.2's diagram draws Preserved -> RecoveredLive; §2.2:125 and §5.8 both forbid it.
Two normative passages against one glyph: the diagram edge is NON-NORMATIVE. Annotated in the
frozen doc; the shipped transition table follows the prose (Preserved/PreservationUnknown terminal
for R2f1b; RecoveredLive's outbound edges are 2c/2d additive work).
