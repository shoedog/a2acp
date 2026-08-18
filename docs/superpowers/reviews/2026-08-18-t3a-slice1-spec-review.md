1. BLOCKER — Section: Population projection
Issue: Claim-bearing ProtectionPrepared records are schema-valid, but the spec defines only the bare form. An implementer could probe a claimed form and return ReadyForLockedReproof, widening the positive population contrary to AC1.
Suggested resolution: Define the complete state×claim table. Require every ProtectionPrepared record, with or without a claim, to refuse before candidate construction, and assign each form an exact refusal reason.

2. BLOCKER — Section: Ownership and API migration
Issue: The proposed proof signature drops the current recovery_owned input while LocallyOwned is declared unwired. A recovery-owned BothAbsent candidate that currently refuses could consequently become ReadyForLockedReproof.
Suggested resolution: Specify the exact old and new signatures and preserve the existing recovery-owned refusal until slice 2 replaces it with an equivalent observer. Define V3 and legacy dispatch separately.

3. BLOCKER — Section: Existing V3 trust boundary
Issue: The specification does not preserve the existing under-root and exact record-file/sibling checks. Dropping either during refactoring could produce ReadyForLockedReproof for an out-of-root or mismatched record while satisfying the listed battery.
Suggested resolution: Require both guards to remain ahead of subject construction, map each failure to a fixed refusal, and add zero-probe regression tests.

4. BLOCKER — Section: Red-first battery
Issue: degraded_materialization_marker_refuses_without_probing has no reproducible behaviorally-red fixture as written. An absent claim is invalid for MaterializationInFlight; an unbound common-directory claim already refuses without probing; a target-degraded but authority-bound claim remains constructible and is probed.
Suggested resolution: Specify degraded claims field-by-field and classify unconstructible degradation as regression coverage. Replace the second genuinely-red case with a constructible claim-bearing ProtectionPrepared record that currently reaches the probe but must newly refuse.

5. BLOCKER — Section: Projection ordering evidence
Issue: Counting ExactAbsenceProbeV1 calls does not prove projection occurs before ExactAbsenceCandidateV1::from_claim. A candidate-first implementation can perform forbidden filesystem and Git authority probes, then reject the state and still pass the counting-probe test.
Suggested resolution: Add an instrumentable construction seam or use an ineligible record with invalid source/common authority whose expected result is IneligiblePopulation; candidate-first code must instead fail distinctly.

6. BLOCKER — Section: Effect-freedom gate
Issue: The named exit test is not required to traverse the production V3 adapter or scanned sweep path. Testing only decide_exact_absence could pass while the adapter writes, renames, or unlinks records or checkout entries.
Suggested resolution: Require all four arms to exercise the real V3 production path with a programmable probe and precisely scoped record-byte and directory-entry snapshots.

7. BLOCKER — Section: Evidence ownership
Issue: The described implement container has no compile loop, yet AC9 and AC10 require exact base-red output and complete post-change gate totals. Dispatching without an assigned external executor makes those criteria impossible to satisfy.
Suggested resolution: Name the controller or host responsible for base-red and final gates, and define how its supplied output is incorporated into the handoff.

8. MAJOR — Section: Refusal completeness
Issue: Root canonicalization failures, unreadable records, record-sibling mismatch, and under-root failures lack a total mapping into the new refusal vocabulary, allowing divergent adapters.
Suggested resolution: Add an exhaustive input-to-refusal table, distinguishing population ineligibility, binding failure, and subject-construction failure.

9. MAJOR — Section: Zero-mutation verification
Issue: Before/after snapshots prove final-state equality but cannot detect a transient write, rename, or unlink followed by restoration.
Suggested resolution: Describe snapshots as final-state evidence and require either an operation-recording seam or source-level verification for the stronger “no mutation operation occurred” claim.

10. MAJOR — Section: Type and legacy decomposition
Issue: The current top-level match cannot directly combine legacy ExactAbsenceDecisionV1 and V3 UnusedCandidateAssessmentV1. The public population enum also advertises ProtectionPrepared as a possible subject despite AC1 making it refusal-only.
Suggested resolution: Specify separate legacy and V3 control-flow branches. Prefer a private projection returning Result<MaterializationInFlightSubject, Refusal>, adding future populations only when they become eligible.

11. MAJOR — Section: Files, handoff, and size cap
Issue: The acceptance criteria require a handoff but name no path, making file scope and numstat accounting indeterminate. The 250-line cap is tight once guard and production-path coverage are added, though not proven impossible.
Suggested resolution: Name the exact handoff file and whether it is created or refreshed, then recalculate the cap after incorporating the required tests and guards.

12. MINOR — Section: Owner decision 1
Issue: The preparation record is described as containing only {flight_id, state}, but it also contains schema_version.
Suggested resolution: Correct the wording without changing the valid conclusion that the record lacks candidate authority.

Disagreement resolved: Rigor is right that construction-order evidence is blocking because AC1 explicitly requires refusal before construction; Soundness is right that the 250-line cap is not yet proven impossible, so sizing remains MAJOR rather than gating.

VERDICT: Not ready to plan; first make ProtectionPrepared exhaustive, preserve ownership and binding guards, repair the genuinely-red battery, prove pre-construction projection and production-path effect freedom, and assign execution of the required evidence gates.