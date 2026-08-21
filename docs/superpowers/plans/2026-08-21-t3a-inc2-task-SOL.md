---
task-type: implement
---

# T3a increment 2 — population admission and construction guards

## Description

Implement T3a increment 2 as two sequential, independently reviewed slices:

1. **Increment 2A:** exhaustive custody-population admission.
2. **Increment 2B:** typed construction guards and exact sibling-name enforcement.

The combined estimate is 755 changed nonblank lines, materially above the inherited 450-line anchor. Do not compress the behavioral evidence to force one candidate under that number. Each selected slice has its own 450-line cap, implementation-candidate commit, frozen genuine-red control, handoff, operator gates, and handoff-only evidence commit.

This specification is self-selecting:

- On the verified base `bade9866278877923de0f247e95d7bd5d813b2b9`, where production constructs neither `IneligiblePopulationV1` nor `CannotConstructSubjectV1`, implement **2A only**.
- Implement **2B only** from a later base where 2A and its operator-evidence commit have landed, the sixteen-population rule is present, and the three placement guards remain untyped.
- Never implement both slices in one candidate. If the base matches neither state, stop before editing and report the exact discrepancy.

### Verified base facts and corrections

The repository at `bade9866278877923de0f247e95d7bd5d813b2b9` establishes:

- `WorktreeCustodyStateV1` has ten variants.
- `PreservationReasonV1::ALL` has six values.
- `ProtectionPrepared` has `ClaimPresenceV1::Optional`; `PreservationPrepared`, `Preserved`, and every `PreservationUnknown` have `Required`; the other six states have `Forbidden`.
- The resulting population count is `2 + 6 + 8 = 16`.
- `report_exact_scan_projection_row` currently renders every readable custody row as `CustodyExactAbsenceAssessmentV1::Assessed(decision)`.
- `EXACT_ABSENCE_POLICY_READY_V1` is `false`.
- The only three `.effective()` call sites are test assertions:
  `exact_absence_sweep_reports_the_stored_runtime_decision`,
  `raw_decision_and_historical_eligibility_projections`, and
  `exact_absence_report_vocabulary_is_public`.
- `entry_is_effectively_authorized_for_policy` requires
  `policy_ready && has_authoritative_scan() && …`; after slice B, a healthy root can satisfy `has_authoritative_scan()`, leaving readiness as the sole remaining production gate.

Two supplied claims require correction:

1. Not every readable custody record currently reaches the exact-absence probe. Records without a claim and records rejected by the current placement or authority checks already stop before it. The actual universal defect is that every readable custody row is reported as `Assessed(decision)`. New zero-probe assertions are genuine runtime red for claim-bearing ineligible populations such as `Preserved`; for bare or claim-forbidden populations, zero probes are characterization while the typed assessment is red.
2. Production does **not** construct `ClaimAuthorityUnavailableV1`. Its only constructor call is in the `sweep/report.rs` unit test `accessors_and_snapshots`. Production `ExactAbsenceCandidateV1::from_claim` performs authority checks, but `decide_unused_custody_record` discards their errors through `.ok()` and collapses them to raw `Refused`. Increment 2B must type that existing failure surface without adding new authority observations.

### Final per-record precedence

This precedence is binding and must appear only once in production:

1. If `record.worktree.canonical_path` is not absolute:
   `CannotConstructSubjectV1::RecordedWorktreePathNotAbsolute`.
2. If its leniently resolved path is not component-wise under the canonical sweep root, or containment cannot be proved:
   `CannotConstructSubjectV1::OutsideSweepRoot`.
3. If the exact descriptor-enumerated child name is not the lexically derived
   `<recorded-worktree-final-component>.custody.v1.json`:
   `CannotConstructSubjectV1::RecordFileNotExpectedSibling`.
4. Apply the exhaustive population-admission table.
5. For an admitted record, construct claim authority. Any existing construction failure becomes
   `CannotConstructSubjectV1::ClaimAuthorityUnavailable(...)`.
6. Only a successfully admitted and constructed subject reaches `observe_exact_absence`, exactly once, and becomes `Assessed(decision)`.

An earlier guard always wins over every later guard. Population admission therefore does not hide malformed placement, but it must run before claim-authority work and before the exact-absence probe.

During 2A, preserve the existing placement checks ahead of admission. Their failures may retain the inherited raw `Assessed(Refused)` representation until 2B types them; do not reorder admission ahead of them merely to simplify the first slice.

### Exact sibling-name rule

Derive the expected child name lexically from the recorded absolute worktree path’s final component and `CUSTODY_RECORD_SUFFIX`. Compare that `OsStr` exactly with `CheckedScanRowV1::parts().1`.

Do not canonicalize either name for this decision. Do not reconstruct the observed name from `record_path`. The current canonical-equality check accepts a wrong regular record when the expected sibling path is a symlink back to that record.

The red fixture must use:

- a valid in-root target and complete, observable claim authority;
- a wrong regular custody filename containing the decoded record;
- the expected sibling filename as a symlink to that wrong regular file;
- the wrong regular filename as the enumerated row.

The current implementation canonicalizes the two paths to the same object and reaches the probe. Increment 2B must report `RecordFileNotExpectedSibling` with zero probe calls. A corrected exact-name control must reach the probe once.

This rule does not repair `is_custody_record_name`’s Unix-only separator guard. The A2a-2 backslash characterization remains unchanged.

### Exhaustive admission table

Use one exhaustive match over `WorktreeCustodyStateV1`. Name every `PreservationReasonV1` explicitly; no wildcard or catch-all is permitted.

| # | Canonically decoded state | Claim | Result after placement guards |
| ---: | --- | --- | --- |
| 1 | `ProtectionPrepared` | absent | `IneligiblePopulationV1::BareProtectionPrepared` |
| 2 | `ProtectionPrepared` | present | Continue to claim-authority construction |
| 3 | `UnusedSettled` | absent, as required by `Forbidden` | `IneligiblePopulationV1::StateNotCandidate` |
| 4 | `Materializing` | absent, as required by `Forbidden` | `IneligiblePopulationV1::StateNotCandidate` |
| 5 | `LiveProtected` | absent, as required by `Forbidden` | `IneligiblePopulationV1::StateNotCandidate` |
| 6 | `PreservationPrepared` | present, as required | `IneligiblePopulationV1::StateNotCandidate` |
| 7 | `Preserved` | present, as required | `IneligiblePopulationV1::StateNotCandidate` |
| 8 | `DeleteAuthorized` | absent, as required by `Forbidden` | `IneligiblePopulationV1::StateNotCandidate` |
| 9 | `Removed` | absent, as required by `Forbidden` | `IneligiblePopulationV1::StateNotCandidate` |
| 10 | `RecoveredLive` | absent, as required by `Forbidden` | `IneligiblePopulationV1::StateNotCandidate` |
| 11 | `PreservationUnknown { NodeFailure }` | present, as required | `IneligiblePopulationV1::StateNotCandidate` |
| 12 | `PreservationUnknown { Cancellation }` | present, as required | `IneligiblePopulationV1::StateNotCandidate` |
| 13 | `PreservationUnknown { AmbiguousCleanup }` | present, as required | `IneligiblePopulationV1::StateNotCandidate` |
| 14 | `PreservationUnknown { MaterializationInFlight }` | present, as required | Continue to claim-authority construction |
| 15 | `PreservationUnknown { PostConditionDisagreement }` | present, as required | `IneligiblePopulationV1::StateNotCandidate` |
| 16 | `PreservationUnknown { RemovalFailed }` | present, as required | `IneligiblePopulationV1::StateNotCandidate` |

The totals are exactly two candidate populations and fourteen ineligible populations.

Do not add `InvalidStateClaimPair` or any equivalent vocabulary. The canonical decoder already rejects:

- a required-claim state without a claim as `CustodyRecordDecodeErrorV1::ClaimRequired`;
- a forbidden-claim state with a claim as `CustodyRecordDecodeErrorV1::ClaimForbidden`.

Through the production checked scan, both remain
`ExactAbsenceRecordAssessmentV1::UnreadableCustody(CustodyReadRefusalV1::Decode(...))`
and make zero probe calls.

### Increment 2A — population admission

Implement a production assessment step before the probe. It must retain both:

- the typed custody assessment used by the report; and
- its raw `UnusedCandidateDecisionV1` projection used by existing logging and private characterization tests.

Do not probe in `report_exact_scan_projection_row`, recompute an assessment there, or filter a previously probed result. The report projection must consume the assessment stored by `project_exact_scan_result`.

Required 2A tests:

- `exact_absence_population_admission_covers_all_sixteen_populations_before_probe`
  exercises all sixteen rows through the production assessment route, checks the exact state snapshot and typed result, and checks zero probe calls for every ineligible row.
- Within that test or a distinct focused test, canonical `Preserved` with complete valid claim authority and a `BothAbsent` recording probe must change from `Assessed(Authorized)` on the untouched base to `StateNotCandidate`, with zero probe calls.
- `candidate_populations_reach_exact_absence_probe_once` proves claim-bearing `ProtectionPrepared` and
  `PreservationUnknown { MaterializationInFlight }` each pass all earlier checks, reach the same recording probe exactly once, and retain the probe’s `Authorized` or `Refused` decision as `Assessed`.
- `invalid_state_claim_pairs_remain_unreadable_without_probe` writes canonical-shaped persisted bytes for at least one required-without-claim record and one forbidden-with-claim record, routes them through the production checked scan, and proves the exact decode refusal plus zero probes.
- Amend the existing `exact_absence_sweep_reports_the_stored_runtime_decision` assertion for its `LiveProtected` row from `Assessed(Refused)` to `IneligiblePopulation(StateNotCandidate)`. This is the only existing decision assertion expected to change in 2A: `LiveProtected` is claim-forbidden and is no longer an admitted population.
- Preserve `exact_projection_retains_production_computed_decisions` and
  `exact_projection_preserves_legacy_and_custody_decision_matrix`: their custody fixture is claim-bearing `ProtectionPrepared`, one of the two admitted populations.

The 2A handoff must name `exact_absence_sweep_reports_the_stored_runtime_decision` and the justification above. It must explicitly state that no other pre-existing decision assertion changed.

### Increment 2B — typed construction guards

Replace the inherited collapsed placement refusals with the first three typed results in the fixed precedence. Admission behavior from 2A must remain unchanged.

Type the existing `ExactAbsenceCandidateV1::from_claim` failure surface instead of discarding it. Preserve the public report vocabulary and existing observations. A private typed construction helper may be added while keeping the existing public constructor behavior available to its current callers.

Use these mappings:

| Existing construction failure | Typed result |
| --- | --- |
| Source outer/embedded path disagreement | `ClaimAuthorityUnavailable(Source, PathMismatch)` |
| Common-directory outer/embedded path disagreement | `ClaimAuthorityUnavailable(CommonDirectory, PathMismatch)` |
| Worktree outer/embedded path disagreement | `ClaimAuthorityUnavailable(Worktree, PathMismatch)` |
| Non-absolute source or common-directory path | Matching object with `NotAbsolute` |
| Missing claim `dev`/`ino` required by current source/common-directory construction | Matching object with `IdentityIncomplete` |
| Source or common-directory identity cannot be observed | Matching object with `ObservationUnavailable` |
| Observed source or common-directory identity differs from the claim | Matching object with `IdentityChanged` |
| Git common-directory binding cannot be observed | `ClaimAuthorityUnavailable(SourceCommonDirectoryBinding, ObservationUnavailable)` |
| Observed Git common-directory binding differs from the claimed authority | `ClaimAuthorityUnavailable(SourceCommonDirectoryBinding, IdentityChanged)` |

The outer recorded worktree absolute-path guard wins before a claim-level worktree `NotAbsolute` result. Do not manufacture an unreachable decoder-bypass test merely to construct the latter combination.

`ClaimAuthorityObjectV1::Root` and `ClaimAuthorityUnavailableReasonV1::OwnershipUnproven` remain vocabulary-only here. Adding retained-root authority checks, ownership inventories, another probe method, or a new Git observation belongs to later work. If truthful typed mapping of the existing construction surface requires any such expansion, stop and report rather than silently importing it.

Required 2B tests:

- `custody_subject_guard_precedence_is_stable` covers at least:
  relative worktree plus wrong sibling; outside-root worktree plus wrong sibling; valid in-root worktree plus wrong sibling and unavailable authority; valid placement plus ineligible population and unavailable authority; and valid admitted placement plus unavailable authority. Assert the exact first result and zero exact-absence probes.
- `expected_sibling_symlink_alias_is_refused_before_authority_or_probe` uses the required red fixture and asserts `RecordFileNotExpectedSibling`.
- `valid_guard_controls_reach_candidate_and_probe_once` corrects each isolated placement fixture and proves successful candidate construction followed by exactly one exact-absence probe.
- `claim_authority_failure_is_reported_as_cannot_construct` exercises each reachable mapping category above and proves zero exact-absence probes.
- A candidate fixture with all guards and existing authority checks satisfied still produces the same `Assessed` decision as 2A.

No existing decision assertion is expected to change in 2B. Its handoff must state that explicitly. Any pre-existing test colour change requires an individual behavioral explanation; otherwise stop and report.

### Frozen genuine-red controls

Each slice creates its own test-only patch:

- 2A:
  `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc2a-genuine-red-control.patch`,
  against `bade9866278877923de0f247e95d7bd5d813b2b9`.
- 2B:
  `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc2b-genuine-red-control.patch`,
  against the exact merged 2A operator-evidence base recorded at 2B preflight.

Each patch must:

- change test code only;
- be generated from and apply cleanly to its recorded base;
- contain a focused `inc2a_control_` or `inc2b_control_` selector;
- record its full SHA-256 in the corresponding handoff;
- record the exact base commit;
- record these repository-relative reproduction commands:

```text
git apply docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc2a-genuine-red-control.patch
CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked inc2a_control_ -- --nocapture
```

For 2B, substitute `inc2b` in both lines.

The implementer does not run these cargo commands. The handoff must classify which control assertions are genuine runtime red and which are characterization. At minimum, 2A’s claim-bearing `Preserved` decision/probe-count oracle and 2B’s symlink-alias typed refusal must be genuine runtime red on their respective untouched bases.

### Readiness fence

`EXACT_ABSENCE_POLICY_READY_V1` remains `false`. Do not change the body or visibility of `effective()` or `entry_is_effectively_authorized_for_policy`.

The comment above the constant currently says increment 2 may change readiness; that comment is stale under the owner ruling. A comment-only correction may assign activation to the T3b slice that first adds a production consumer of `effective()`. No report type, variant, field, accessor, or public signature may change.

The reason is part of the contract: readiness currently has zero production consumers, so enabling it changes no production behavior, while slice B made it the sole remaining eligibility gate for a healthy root. Removing the last guard now buys nothing. T3b must activate and review it alongside its first real consumer.

### Scope fences

Neither slice may:

- change the public signature
  `fn(&str, &dyn ExactAbsenceProbeV1) -> ExactAbsenceSweepReportV1`
  of `sweep_orphans_with_exact_absence`;
- change the public vocabulary in `sweep/report.rs`;
- change `classify_root_observations`, `RootObservationSetV1`, the retained-descriptor enumerator, or slice B’s root behavior;
- change the legacy assessment or destructive `sweep_orphans` path;
- add ownership, locking, transitions, settlement, unlink, pruning, checkout removal, or publication;
- add or move Git observations beyond those already performed by `ExactAbsenceCandidateV1::from_claim`;
- repair `is_custody_record_name`;
- change a manifest, lockfile, or dependency.

T3a remains decision-and-report only. Any later actor must re-open the root, re-read the exact enumerated record, re-bind every identity, reapply admission, and re-prove exact absence under its own lock before acting.

### Characterization that must remain stable

All ten A2a-2 scenarios named in
`docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2a-2-handoff.md`
must retain their behavior. All slice B root-observation tests must retain their behavior. Legacy rows and unreadable custody rows must retain their ordering and raw decision projections.

Do not update an expectation merely because a test is red. Only the one named 2A assertion is pre-authorized to change.

### Commit and handoff protocol

For each selected slice:

1. Record pre-edit `HEAD`, branch, `git status --short`, and the anchor audit in the slice handoff.
2. Implement only the selected slice.
3. Create the frozen control and record its base and SHA-256.
4. Record final per-row and total changed-line counts.
5. Author the handoff with the following six lines exactly, unticked and without invented results:

```text
- [ ] PENDING OPERATOR — `cargo fmt --all -- --check`
- [ ] PENDING OPERATOR — `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] PENDING OPERATOR — `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast`
- [ ] PENDING OPERATOR — `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast`
- [ ] PENDING OPERATOR — `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point
- [ ] PENDING OPERATOR — `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point
```

6. Make exactly one implementation-candidate commit containing production, tests, the frozen control, and the pending handoff.
7. Stop. The host operator owns all six gates, the same-environment base control for any attributed failure, final totals, and the later handoff-only evidence commit.

The implement container’s inability to fetch pinned `a2a-lf` is an expected gate block. Do not retry with network access and do not report fabricated totals.

### Sizing worksheet and stop rules

Count nonblank physical additions plus deletions against the selected slice’s exact base. Include production, tests, frozen control, and handoff. A modified line counts once as a deletion and once as an addition.

The inherited combined 450-line estimate is falsified by this worksheet:

| Slice/component | Estimate | Cap |
| --- | ---: | ---: |
| 2A production assessment storage, admission, and projection wiring | 55 | 75 |
| 2A reusable recording-probe and valid-authority fixtures | 35 | 50 |
| 2A exhaustive sixteen-population test | 105 | 125 |
| 2A admitted-population once-only controls | 45 | 60 |
| 2A invalid persisted-pair production-route test | 35 | 50 |
| 2A existing assertion and readiness-comment amendments | 5 | 15 |
| 2A frozen genuine-red control | 55 | 70 |
| 2A handoff | 35 | 50 |
| **2A total** | **370** | **450** |
| 2B production placement guards and exact child-name comparison | 55 | 75 |
| 2B typed mapping of the existing authority-construction surface | 45 | 65 |
| 2B guard and multi-failure precedence matrix | 75 | 95 |
| 2B symlink-alias red fixture and corrected control | 45 | 60 |
| 2B reachable authority-mapping tests | 35 | 50 |
| 2B characterization amendments | 5 | 15 |
| 2B frozen genuine-red control | 55 | 70 |
| 2B handoff | 70 | 80 |
| **2B total** | **385** | **450** |
| **Combined increment 2** | **755** | **Not a permitted single candidate** |

Before editing, re-estimate every row against the selected base. After editing, replace estimates with measured counts.

If a row exceeds its cap, the selected-slice total would exceed 450, an additional production file is required, or adequate behavioral evidence will not fit, stop and report a revised split. Do not shorten tests, omit negative cases, combine semantically distinct assertions into an unreadable oracle, or drop the frozen control to meet a cap.

## Acceptance Criteria

1. Exactly one of 2A or 2B is selected from repository state and only that slice is implemented.
2. The final production flow follows the six-step precedence exactly.
3. The admission match names all sixteen populations without a wildcard and yields exactly two candidates and fourteen ineligible populations.
4. Every ineligible population is refused before claim-authority construction and before the exact-absence probe; zero-probe assertions cover all fourteen.
5. Canonical `Preserved` with complete claim authority and `BothAbsent` is `StateNotCandidate` with zero probes.
6. Bare `ProtectionPrepared` is `BareProtectionPrepared`, not malformed and not a missing-claim result.
7. Claim-bearing `ProtectionPrepared` and materialization-in-flight unknown each reach the exact-absence probe exactly once when all earlier checks pass.
8. Invalid required/forbidden claim pairs remain exact `UnreadableCustody(Decode(...))` results with zero probes; no invalid-pair report arm exists.
9. Increment 2B populates all three placement-guard arms with the fixed first-failure precedence and exact `OsStr` child-name comparison.
10. Increment 2B converts the existing reachable claim-authority construction failures to the specified `ClaimAuthorityUnavailable` object/reason pairs without adding observations.
11. The expected-sibling symlink-alias fixture reports `RecordFileNotExpectedSibling` with zero probes, while its corrected control probes once.
12. The raw decision used for logging is derived from the stored typed assessment; neither report construction nor logging re-probes or re-decides.
13. `exact_absence_sweep_reports_the_stored_runtime_decision` is the only pre-existing decision assertion changed by 2A, with the required justification; 2B changes none.
14. The ten A2a-2 scenarios, slice B root tests, legacy behavior, unreadable-custody behavior, ordering, and scan status remain unchanged.
15. `EXACT_ABSENCE_POLICY_READY_V1` remains `false`; `effective()` and its private policy predicate remain behaviorally unchanged; the stale readiness comment may be corrected.
16. No public report vocabulary or sweep signature changes.
17. No mutation, action authority, ownership, locking, transition, dependency, manifest, lockfile, classifier, enumerator, or separator-guard change is present.
18. The selected slice carries a test-only frozen red patch, exact base, SHA-256, repository-relative run command, and truthful evidence classification.
19. The selected handoff carries the exact six unticked `PENDING OPERATOR` lines, the pre-edit audit, changed-test accounting, measured sizing worksheet, and no invented gate result.
20. Every sizing row and the selected-slice total are within cap, or the implementation stopped and reported before compressing evidence.
21. Exactly one implementation-candidate commit exists for the selected slice; the operator-owned evidence commit is absent when the implementer stops.

## Files

- `crates/bridge-worktree/src/sweep.rs`
  - 2A: assessment storage, exhaustive admission, report projection, and tests.
  - 2B: typed placement guards, exact child-name matching, bounded typed authority mapping, and tests.
- `crates/bridge-worktree/src/sweep/report.rs`
  - Read-only except the comment above `EXACT_ABSENCE_POLICY_READY_V1`.
  - No vocabulary, accessor, readiness value, or predicate change.
- `crates/bridge-worktree/src/sweep/checked_scan.rs`
  - Prefer read-only. Its existing `enumerated_name` seam already supplies exact child identity.
  - Test-only fixture changes are allowed only if the production projection cannot otherwise be exercised.
- `crates/bridge-worktree/src/custody.rs`
  - Read-only source of the state/reason enums, claim-presence rules, decoder behavior, suffix, and record reader.
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs`
  - Read-only; public signatures and vocabulary remain settled.
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc2a-genuine-red-control.patch`
  - Create in 2A only.
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc2a-handoff.md`
  - Create in 2A only.
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc2b-genuine-red-control.patch`
  - Create in 2B only.
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc2b-handoff.md`
  - Create in 2B only.
- `Cargo.toml`, `Cargo.lock`, and every crate manifest
  - Must remain byte-for-byte unchanged.

## Spec Refs

- `crates/bridge-worktree/src/sweep.rs`
  - `ExactAbsenceCandidateV1::from_claim`
  - `worktree_under_root`
  - `decide_unused_custody_record`
  - `ExactScanProjectionRowV1`
  - `report_exact_scan_projection_row`
  - `project_exact_scan_result`
  - `sweep_orphans_with_exact_absence`
- `crates/bridge-worktree/src/sweep/report.rs`
  - `EXACT_ABSENCE_POLICY_READY_V1`
  - `ExactAbsenceSweepReportV1::effective`
  - `entry_is_effectively_authorized_for_policy`
  - `CustodyExactAbsenceAssessmentV1`
  - `IneligiblePopulationV1`
  - `CannotConstructSubjectV1`
  - `ClaimAuthorityUnavailableV1`
- `crates/bridge-worktree/src/custody.rs`
  - `PreservationReasonV1`
  - `WorktreeCustodyStateV1`
  - `WorktreeCustodyStateV1::claim_presence`
  - `WorktreeCustodyRecordV1::validate`
  - `CustodyRecordDecodeErrorV1`
  - `CustodyReadRefusalV1`
  - `custody_record_path`
  - `is_custody_record_name`
- `crates/bridge-worktree/src/sweep/checked_scan.rs`
  - `CheckedScanRowV1::parts`
  - `scan_checked_rows_with_source`
  - the existing recording probe and scripted source tests
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs`
  - public vocabulary and exact function-signature assertions
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2a-2-handoff.md`
  - the ten preserved characterization scenarios and separator divergence
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-handoff.md`
  - report-return custody and frozen-control protocol
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc1-sliceB-handoff.md`
  - authoritative-root behavior, readiness consequence, sizing, and operator-evidence protocol

## Commit Message

feat(worktree): enforce exact-absence custody assessment boundaries

Apply the repository-selected T3a increment 2 slice without broadening its
authority. Narrow readable custody records through exhaustive population
admission or through the typed construction guards that precede it, preserving
the raw decision projection, checked-scan behavior, and decision-only T3a/T3b
boundary.

Keep exact-absence policy readiness false. Add no mutation, locking, ownership,
dependency, manifest, or public report-vocabulary change.
