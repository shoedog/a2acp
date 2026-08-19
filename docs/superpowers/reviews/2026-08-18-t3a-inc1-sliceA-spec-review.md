BLOCKER

1. WRONG — Section: Scan flow step 4 / behavior preservation.
Issue: Processing each row before requesting the next name reverses today’s enumerate-and-read-all, then probe-and-log ordering. If probing row A creates, removes, or rewrites row B, the specified implementation observes a different result.
Suggested resolution: Preserve two phases—enumerate/read all intermediate rows, finish enumeration, then assess and log—or explicitly declare the concurrency-semantics change and provide behavioral evidence.

2. WRONG — Section: Public vocabulary / Acceptance 1.
Issue: The spec names fifteen public types while requiring fourteen, leaves several variants, payloads, fields, derives, conversions, and accessors undefined, and simultaneously makes CustodyRootObservationV1 public while declaring all observation types crate-private. Implementers can produce incompatible APIs, and slice B may require a breaking change.
Suggested resolution: Supply literal Rust declarations or exhaustive equivalent tables for every public type; correct the count; define all assessment mappings and construction signatures; and explicitly distinguish public CustodyRootObservationV1 from private raw observation types.

3. WRONG — Section: Scanner seam / classifier contract.
Issue: RootObservationSetV1 and “every required observation” are undefined. An implementation using only two snapshots could report Pinned even though enumeration, custody reads, and final named-root observation refer to different filesystem objects.
Suggested resolution: Define the retained enumerator identity, pinned custody-directory identity, final no-follow named-root observation, capture times, identity-comparison rules, usability rules, and complete precedence for Pinned, IdentityChanged, and Unavailable.

4. WRONG — Section: Compatibility scanner open behavior.
Issue: Pin-creation failure is not assigned a result. With successful read_dir but failed PinnedDirectoryV1 creation, returning CannotEnumerate would omit rows and logs that current behavior preserves: legacy rows still proceed, while custody rows become UnreadableCustody.
Suggested resolution: Specify that open refuses only when read_dir fails, retains any pin failure in session state, and reproduces today’s per-entry legacy and custody outcomes.

5. WRONG — Section: Public report / Acceptance 2.
Issue: is_authoritative() lacks a truth table. For an incomplete enumeration with stable root identity, “Pinned alone” returns true while “Complete and Pinned” returns false, producing incompatible authority decisions.
Suggested resolution: Define it explicitly—presumably enumeration is Complete and custody-root observation is Pinned—and add acceptance cases for every scan/root combination.

MAJOR

6. SMELL — Section: Effective projection API.
Issue: effective_decision_at(index) separates a decision from its governing entry; filtering or reordering entries can apply row A’s Authorized result to row B.
Suggested resolution: Prefer an accessor or iterator returning the entry and effective decision as a bound pair, and distinguish raw observations from action-capable decisions through type or visibility.

7. SMELL — Section: Split sequencing.
Issue: After slice B can produce Pinned, Complete + Pinned + Preserved + BothAbsent becomes effectively Authorized before increment 2 installs its refusing admission rule.
Suggested resolution: Keep an internal policy-readiness gate false until admission lands, or explicitly prohibit consumers of effective authorization during the stacked interval.

8. SMELL — Section: Acceptance 17 sizing.
Issue: The 600-line cap has little credible margin for fifteen public types, projections, scanner implementations, classifier, traversal refactor, characterization tests, seam tests, and the approximately 97-line handoff.
Suggested resolution: Add a per-component line budget or split characterization/closure before dispatch; retain the mandatory pre-edit stop rather than compressing evidence.

9. SMELL — Section: Scan status and characterization precision.
Issue: “Canonicalizes once” does not name canonicalize_lenient, so a missing root could be tested as CannotCanonicalize instead of today’s CannotEnumerate. The positive legacy row also omits the condition that source/common authority must construct a candidate.
Suggested resolution: Name the exact helper and qualify the legacy probe-mapping row with successful candidate construction.

MINOR

10. SMELL — Section: Platform instructions.
Issue: “No platform gating” conflicts in wording with required non-Unix dead-code allowances and Unix-only fixtures.
Suggested resolution: Clarify that production functionality must not be excluded, while lint-only cfg_attr annotations and inherently Unix-only tests remain permitted.

11. SMELL — Section: Seam evidence.
Issue: “Zero log-helper calls” names no existing helper or injected interface; production currently logs through tracing.
Suggested resolution: Define the observable as zero matching tracing events or introduce a private reporter seam used by production.

12. SMELL — Section: Handoff acceptance.
Issue: The required “marked operator-evidence section” has no literal heading or placeholder syntax.
Suggested resolution: Provide the exact marker and placeholders the implementer must preserve.

Lens resolution: There is no substantive conflict; Soundness’s explicit three-authority observation model is adopted as the stronger resolution of Rigor’s undefined-observation blocker, and both lenses agree the characterization matrix—including raw Authorized for the load-bearing Preserved/BothAbsent row and silent malformed-legacy omission—is correct at 9aedf175.

VERDICT: Not ready to plan; first fix the five BLOCKER items above.