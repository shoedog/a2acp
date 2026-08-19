Prior-round adjudication:
- FIXED — Rust enum-field privacy was corrected to apply only to structs.
- FIXED — ExactAbsenceProbeV1’s one-method shape and production-only effect-freedom scope were corrected.
- PARTIAL — The vacuous “existing tests stay green” claim was replaced by a characterization matrix, but several fixtures and literal expectations remain unspecified.

Disagreements resolved: Soundness is right that a runtime base-red exists for the return-value/API change, though not for decision behavior; Soundness is also right to treat the stale 300-line review criterion as non-gating because task AC16 establishes the operative 500-line cap.

1. BLOCKER — ExactAbsenceEnumerationV1 / legacy parsing — WRONG. A malformed legacy sidecar is currently omitted, but the spec requires it to appear while providing no assessment variant capable of representing it. An implementer must either violate behavior preservation, misclassify the row, or violate the frozen taxonomy. Resolve by preserving omission in increment 1 or adding an explicit unreadable-legacy outcome with defined projection, entry, and logging behavior.

2. BLOCKER — CustodyRootObservationV1 — WRONG. Before/after path observations do not bind the directory actually enumerated or used for custody reads. An A→B→A replacement can therefore report Pinned while combining rows from different root objects; on non-Unix, absent dev/inode identity makes same-name replacement particularly unprovable. Require usable stable object identity, retain one root object for enumeration and record reads, and specify every open, its order, and its descriptor consumer. Otherwise return Unavailable or another explicitly non-authoritative status.

3. BLOCKER — Checked scanner contract — WRONG. Result structs are specified without the scanner signature, canonicalization ownership, refusal mapping, compatibility-wrapper adaptation, or streaming behavior. Implementations can consequently canonicalize the legacy wrapper, duplicate scans, change record paths, or collect names before reading records and alter concurrent-replacement behavior. Specify one exact streaming API and the complete mapping for both canonical-root and raw-root entry points.

4. BLOCKER — Frozen public API — SMELL. CustodyStateSnapshotV1 lacks literal fields, types, accessors, conversion mapping, and required trait surface, while other accessors are also underspecified. Different implementations can expose incompatible APIs that increment 2 cannot consume without revision. Provide complete definitions and accessor signatures for every frozen public type.

5. BLOCKER — Deferred taxonomy — WRONG. ClaimAuthorityUnavailable is frozen as a unit variant while increment 3 is promised a typed object/reason product. Adding that payload later breaks construction and exhaustive matching of the V1 enum. Carry an evolvable private-field payload struct now or explicitly commit increment 3 to a new versioned assessment type.

6. BLOCKER — Characterization matrix — SMELL. Load-bearing rows remain “today’s result,” and their outcomes vary with probe results and claim shape. Two implementers can write materially different matrices while satisfying the prose. Specify each complete fixture, probe result, and literal expected decision, or define the required Cartesian product.

7. BLOCKER — Root-observation evidence — WRONG. The spec permits IdentityChanged and failure arms to remain unexecuted, allowing reversed comparisons or incorrect Pinned mappings to pass. Require an injected observation seam with deterministic coverage of Pinned, IdentityChanged, and both before/after observation failures.

8. BLOCKER — Effect-freedom audit — WRONG. The allowed-leaf list permits only bounded reads, but the current path includes read_dir traversal and legacy std::fs::read without stated bounds. The required audit therefore cannot truthfully satisfy its own whitelist. Explicitly admit these existing read-only leaves or separate mutation-freedom from resource-boundedness.

9. BLOCKER — Red-first battery — WRONG. A genuine runtime base-red exists for the intended API change: bind the production result and assert std::mem::size_of_val(&report) > 0; it compiles on both versions, fails for the current unit return, and passes for the report. Distinguish “no decision-behavior red” from “no API-observability red” and require this regression evidence.

10. MAJOR — Projection / root status — WRONG. entry.assessment().decision() may return Authorized while the enclosing report says IdentityChanged or Unavailable. Freeze a report-level effective-decision projection, or specify the exact increment-2 conjunction and how root failure becomes a refusal without changing the frozen API.

11. MAJOR — Report identity — WRONG. CheckedScanRowV1 retains OsString, but the public entry exposes only a lossy String path, so distinct non-UTF-8 names can become indistinguishable. Retain an exact OsString or PathBuf identity in the report and use String only for display-compatible logging.

12. MAJOR — Checked scanner exposure — SMELL. Public CheckedScanV1 and CheckedScanRowV1 freeze an internal intermediate coupled to ScannedWorktreeRecordV1. Keep the streaming checked scanner crate-private unless an identified external consumer requires that surface.

13. MAJOR — Sizing — SMELL. The 500-line allocation totals exactly 500, undercounts thirteen public types as eleven, and leaves no room for two seams or corrections. Raise the ceiling with justification or reduce the public scanner surface and use shared table-driven fixtures.

14. MINOR — Size criterion — SMELL. The outer review criterion still says 300 lines while task AC16 says 500. Update the stale review criterion so closure applies one threshold.

15. MINOR — Construction wording — SMELL. “Never constructs” conflicts literally with the required projection tests. Change it to “production never constructs” and expressly permit test construction.

16. MINOR — Source compatibility — SMELL. The handoff requirement mentions only #[must_use], but the return-type change can also break function-pointer, closure-return, and explicit-unit consumers. Document that complete compatibility boundary.

Verdict: not ready to plan; resolve blockers 1–9 first.