PRIOR-ROUND ADJUDICATION

- FIXED — Public type count: corrected from fourteen to fifteen.
- FIXED — Policy-readiness gate: added and held false until increment 2.
- PARTIAL — Two-phase traversal: production ordering is corrected, but the seam-test wording still permits assessment/logging during enumeration.
- PARTIAL — Pin failure: required compatibility behavior is corrected, but no deterministic mechanism proves the compatibility implementation preserves it.
- PARTIAL — Observation visibility: the public/raw distinction is explained correctly, but contradicted later by “all seam and observation types stay crate-private.”
- The other prior-round findings are not individually identified in the supplied reviews, so they cannot be separately adjudicated.

1. BLOCKER / WRONG — Public vocabulary / Acceptance 1. Issue: The spec requires a frozen, literal fifteen-type API while supplying only names and selected fragments. Two compliant implementers could choose incompatible variants, payloads, fields, accessors, or conversions, forcing slice B or increment 2 into a breaking redesign. Suggested resolution: Include the complete Rust declarations in the spec or incorporate a separately reviewed API appendix verbatim.

2. BLOCKER / WRONG — Raw and effective projection. Issue: The spec forbids `effective_decision_at` and requires `effective()`, but later directs future action code to call `effective_decision_at`; following either instruction violates another acceptance requirement. Suggested resolution: Replace every stale reference, including provenance and action guidance, with `effective()` or “the effective entry/decision pair.”

3. BLOCKER / WRONG — Scanner visibility. Issue: `CustodyRootObservationV1` is required to be public, while the scanner section says all observation types are crate-private; an implementer could hide a required public type. Suggested resolution: Say “all seam and raw observation types stay crate-private” consistently.

4. BLOCKER / WRONG — Bound-pair safety claim. Issue: `effective()` returns a tuple containing a by-value, `Copy` decision, so consumers can collect decisions separately, reorder entries, and apply one row’s authorization to another. The claimed invariant that decisions “can never be separated” is therefore false. Suggested resolution: Return an opaque effective-entry/action-capability view that retains row identity, or explicitly downgrade row binding to a convention and specify the later action-side identity check.

5. BLOCKER / WRONG — Scan flow / seam tests. Issue: “Each row processed before the next `next_name` call” can require assessment and logging during enumeration, contradicting the mandatory eager two-phase flow and allowing Git probes to observe state before enumeration is drained. Suggested resolution: Require two explicit assertions: each yielded record is read and collected before the next name, and no assessment, probe, or decision event occurs until `next_name` returns `None` and `finish` completes.

6. BLOCKER / WRONG — Compatibility pin-failure evidence. Issue: The generic fake scanner can remain green even if the real compatibility source converts pin failure into an open refusal, which would omit legacy and custody rows and their logs. Suggested resolution: Inject a deterministic pin opener into the compatibility source, or provide another discriminating test mechanism that exercises successful `read_dir` followed by pin failure in that implementation.

7. BLOCKER / WRONG — Acceptance 17 / sizing. Issue: The outer criterion requires 600 changed lines, the task says 700, and the component estimates total about 760. The mandatory pre-edit rule therefore requires the implementer to stop before editing. Suggested resolution: Re-slice the work or establish one authoritative cap and an explicit waiver supported by a credible component estimate beneath it.

8. MAJOR / SMELL — Scanner seam / classifier declarations. Issue: `CheckedScanOpenRefusalV1`, `CheckedScanEntryRefusalV1`, `RootObservationSetV1`, and the identity-capture representation are referenced but not defined. In particular, an incautious reuse of `DirectoryIdentityV1::matches` could treat unavailable birthtime differently from the required `Unavailable` result. Suggested resolution: Specify the private declarations and an explicit comparison over complete `(dev, ino, birthtime)` captures.

9. MAJOR / SMELL — Public safety boundary. Issue: Public `is_authoritative()` can be combined with raw `decision()`, allowing future code to bypass policy readiness even after the tuple-binding problem is fixed. Suggested resolution: Separate raw reporting from action eligibility and rename `is_authoritative()` to express scan authority rather than action authority.

10. MAJOR / SMELL — Slice boundary. Issue: Slice A includes the complete three-capture classifier even though production supplies an empty observation set and pinned-root classification is assigned to slice B. Suggested resolution: Defer the private classifier and its non-production tests to slice B unless slice B demonstrably needs its exact internal shape landed first.

11. MAJOR / SMELL — Decomposition. Issue: Adding fifteen public types, policy projections, scanner abstractions, traversal, compatibility behavior, and extensive tests to the existing large `sweep.rs` increases coupling and review burden. Suggested resolution: Consider a report/vocabulary module plus a private checked-scanner module without expanding `bridge-core` surface.

12. MINOR / SMELL — Slice-B exclusions. Issue: “No root pinning” is imprecise because compatibility behavior retains the existing `PinnedDirectoryV1` custody-read pin. Suggested resolution: Say slice A adds no authoritative enumeration-root pinning beyond the existing custody-read pin.

Disagreement resolution: The Rigor lens is right to gate the tuple-binding and two-phase-test defects because each admits a concrete wrong authorization or behavior change; the Soundness lens is right that classifier placement and module decomposition remain non-gating design advice.

VERDICT: NOT READY TO PLAN — first freeze the complete API, remove the contradictory instructions, make row binding and compatibility pin-failure evidence enforceable, clarify two-phase testing, and reconcile the line cap with the estimated scope.