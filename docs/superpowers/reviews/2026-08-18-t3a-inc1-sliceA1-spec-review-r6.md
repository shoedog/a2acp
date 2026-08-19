BLOCKER

1. Section: Prior-round A2 mutation audit — FIXED. Issue: The former false universal claim about non-byte-identical registration fields excluded invalid UTF-8. The revision now records the UTF-8 decode refusal before comparison and requires its raw-Refused report evidence. Suggested resolution: None.

2. Section: Prior-round API evidence — PARTIAL. Issue: The external signature-check function now exists, but `let _ = report.effective();` does not verify the promised `Iterator<Item = &ExactAbsenceSweepEntryV1>` item type. This is not a behavioral or implementation blocker. Suggested resolution: Strengthen the assertion as described under MINOR item 5.

3. Section: Prior-round sizing cap — FIXED. Issue: The 700-line cap is now stated consistently once and has a mandatory pre-edit stop. Suggested resolution: None.

4. Section: Prior-round effective-decision scalar — FIXED. Issue: The revision consistently prohibits `effective_decision_at` and preserves borrowed-entry projection. Suggested resolution: None.

5. Section: Whole specification — No open BLOCKER. Issue: Both lenses confirmed the characterization matrix, including `Preserved + valid claim + vanished target + BothAbsent → Authorized`, malformed-legacy omission, eager two-phase ordering, canonical exact-scan versus raw action-scan separation, and the T3a/T3b authority boundary. Suggested resolution: Proceed with A1.

MAJOR

6. Section: A2 return-type migration — SMELL. Issue: Changing public `sweep_orphans_with_exact_absence` from `()` to `ExactAbsenceSweepReportV1` can break an external caller that stores its old function type or uses it in a unit-constrained expression, although all repository callers are statement-position and safe. Suggested resolution: Record the intentional source break in the A2 handoff, or preserve a unit-returning wrapper and expose the report-returning entry point separately.

7. Section: A2 compatibility/action scanner policy — SMELL. Issue: Two scanner implementations remain a future policy-drift seam even with the shared classifier and conformance matrix. Suggested resolution: Share the internal enumeration/session/read machinery where practical while retaining canonical-versus-raw roots and the distinct flattened-error versus incomplete-status projections.

Disagreement resolution: Soundness is right that the A2 return migration can break external source consumers, while Rigor is right that the audited repository callers are safe; it is therefore MAJOR fix-along advice, not a blocker for A1.

MINOR

8. Section: A1 external API signature evidence — WRONG. Issue: `let _ = report.effective();` proves method visibility but not its exact iterator item type, so the spec overstates that every promised accessor signature is constrained. Suggested resolution: Pass the iterator to a generic helper requiring `Iterator<Item = &ExactAbsenceSweepEntryV1>`, or require `report.effective().next()` to type-check as `Option<&ExactAbsenceSweepEntryV1>`.

9. Section: A1 sizing — SMELL. Issue: The 145-line estimate for the extensive unit matrix plus external API test may be optimistic, though the 700-line cut remains credible. Suggested resolution: Re-estimate tests and the installed-template handoff before editing, and use the existing mandatory stop if the total no longer fits.

VERDICT: READY TO PLAN AND DISPATCH A1; address the exact `effective()` signature assertion and A2 migration/drift advice as non-gating fix-along work.