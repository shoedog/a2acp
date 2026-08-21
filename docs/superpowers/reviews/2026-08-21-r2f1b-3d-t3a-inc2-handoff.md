# T3a increment 2 handoff — population admission and typed placement guards

## Candidate checkpoint

- Base tree: `bade9866278877923de0f247e95d7bd5d813b2b9`.
- Pre-edit `git status --short` was empty. The implementation began from that clean tree.
- This is an implementation candidate only. Cargo gates, frozen-control application, and all runtime results are operator-owned and were not run in this container.
- Re-measurement before edit found 34 `#[test]` markers and 1,282 nonblank lines after markers in `sweep.rs`; the task's 1,316 brace-matched figure includes interleaved helpers. The planned combined increment remained within its component caps, so the split trigger did not fire.

## Pre-edit checkpoint and disposition

| Anchor | Repository evidence | Disposition |
| --- | --- | --- |
| Custody probe path | `sweep.rs`, `decide_unused_custody_record` was the sole custody path to `decide_unused_candidate` | Replaced with `assess_custody_record`; guards and admission precede its only probe call. |
| Row timing | `project_exact_scan_result` completed probing before `report_exact_scan_projection_row` | Move finished assessment construction into the projection loop; report conversion is now a carrier. |
| Population vocabulary | `custody.rs`, the ten state variants, all six `PreservationReasonV1` variants, and `claim_presence()` matched the sixteen-population table | Confirmed; exhaustive admission match names every state and reason. |
| Placement vocabulary | `report.rs`, exactly two `IneligiblePopulationV1` and four `CannotConstructSubjectV1` arms | Confirmed; no public vocabulary was changed. |
| Invalid claim pairs | `WorktreeCustodyRecordV1::decode_canonical` validates `ClaimRequired` and `ClaimForbidden` before scan construction | Confirmed; invalid persisted pairs remain unreadable, with no new invalid-pair arm. |
| Corrected claim-authority assertion | `ClaimAuthorityUnavailableV1::new` appeared only in `report.rs` and `r2f1b_exact_absence_report_api.rs` tests | Re-refuted: no production construction existed and none is added. |
| Corrected placement assertion | The loop in `project_exact_scan_result`, not `report_exact_scan_projection_row`, reaches the custody probe | Re-refuted: post-hoc report filtering cannot suppress a probe, so admission is installed in the custody decision path. |
| Stored exact child name | `CheckedScanRowV1` retains `enumerated_name: OsString` beside lossy `record_path` | Confirmed; guards take `&OsStr`, never the lossy display path. |
| Readiness | `effective()` had only test consumers; healthy roots could be `Pinned` after slice B | Fence retained: readiness remains false and no report API changed. |

Proceed decision: all factual anchors matched the base. The implementation includes 2A and 2B in one candidate; it did not broaden into increment 3 or an action-path change.

## Implementation and scope fences

- `construction_guards` reports, in fixed order, `RecordedWorktreePathNotAbsolute`, `OutsideSweepRoot`, then `RecordFileNotExpectedSibling`. It canonicalizes only the containment check through existing `worktree_under_root`; matching uses the exact enumerated `OsStr`, lexical target file name plus `CUSTODY_RECORD_SUFFIX`, and exact parent equality with the canonical root.
- `admit_custody_population` is exhaustive over `(&WorktreeCustodyStateV1, bool)`. Only claim-bearing `ProtectionPrepared` and `PreservationUnknown { MaterializationInFlight }` admit. Bare `ProtectionPrepared` reports `BareProtectionPrepared`; the other thirteen policy rows report `StateNotCandidate`.
- `assess_custody_record` runs placement guards, admission, then unchanged claim construction and `decide_unused_candidate`. A failed `from_claim` remains `Assessed(Refused)`; `ClaimAuthorityUnavailable` remains dormant for increment 3's typed mapping.
- `ExactScanProjectionRowV1` now carries `ExactAbsenceRecordAssessmentV1`. Per-row logs derive their unchanged decision via `assessment.decision()`, and `report_exact_scan_projection_row` does no probe, filesystem operation, state match, or typed-refusal construction.
- `EXACT_ABSENCE_POLICY_READY_V1` remains `false`; `effective()` and `entry_is_effectively_authorized_for_policy` are untouched. The matched custody control in `a_canonical_preserved_record_stops_before_the_probe_with_a_matched_control` proves a `Pinned` root containing a custody-side `Assessed(Authorized)` entry still has `effective().count() == 0`.
- Changed files: `crates/bridge-worktree/src/sweep.rs`, `crates/bridge-worktree/src/sweep/checked_scan.rs`, this handoff, and the frozen control patch. `Cargo.toml`, `Cargo.lock`, `crates/bridge-core/Cargo.toml`, and `crates/bridge-worktree/Cargo.toml` are unchanged.

## Tests and evidence classification

| Test | Evidence category | Coverage |
| --- | --- | --- |
| `population_admission_covers_every_decodable_population_and_probes_only_candidates` | Genuine runtime red | Sixteen actual records with complete real claim authority; exact assessment table and exactly two probe calls. |
| `a_canonical_preserved_record_stops_before_the_probe_with_a_matched_control` | Genuine runtime red | Canonical `Preserved` is zero-probe; its matched claim-bearing custody control is `Pinned`, reaches once as `Assessed(Authorized)`, and leaves `effective()` empty. |
| `an_expected_sibling_symlink_alias_is_refused_with_a_matched_control` | Genuine runtime red | Types the alias hole, retains the symlink as unreadable, and proves its control probes once. |
| `a_nested_target_whose_record_sits_at_the_root_is_not_an_expected_sibling` | Genuine runtime red | Preserves the mandatory parent component check. |
| `a_target_resolving_outside_the_sweep_root_is_typed_outside_root` | Genuine runtime red | Types containment refusal before sibling matching. |
| `a_relative_recorded_target_reports_the_first_guard_only` | Genuine runtime red | Pins guard-class precedence. |
| `a_preserved_record_outside_the_root_reports_the_guard_not_the_population` | Genuine runtime red | Pins guard-over-admission precedence. |
| `invalid_persisted_claim_pairs_stay_unreadable_and_never_probe` | Characterization | `ClaimForbidden` and `ClaimRequired` remain decode refusals with zero probes. |
| `exact_absence_sweep_reports_the_stored_runtime_decision` | Characterization amendment | The sole pre-existing assessment-colour change. |

The seven named mechanical `ExactScanProjectionRowV1` reads in `checked_scan.rs` now use `row.assessment.decision()` with identical asserted values: `pin_failure_leaves_the_root_observation_unavailable` (its two tuple maps), `checked_scan_reads_each_selected_name_before_next_and_finishes_once`, `exact_route_pin_failure_preserves_legacy_and_refuses_custody`, `exact_projection_retains_production_computed_decisions`, `exact_projection_preserves_legacy_and_custody_decision_matrix`, `unreadable_custody_refuses_without_probe`, and `nondefault_root_observations_survive_exact_without_changing_rows_or_decisions`. They are mechanical carrier reads, not colour changes. `exact_projection_retains_production_computed_decisions` retains its two probe-call assertion.

The one legitimate existing assertion change is `exact_absence_sweep_reports_the_stored_runtime_decision`: its `LiveProtected` custody row changes from `Assessed(Refused)` to `IneligiblePopulation(StateNotCandidate)`. That live, claim-forbidden state is now refused by policy before observation; its state snapshot, raw `decision()`, three-entry report size, authoritative scan, and empty effective iterator remain unchanged.

## Frozen genuine-red control

[`2026-08-21-r2f1b-3d-t3a-inc2-genuine-red-control.patch`](2026-08-21-r2f1b-3d-t3a-inc2-genuine-red-control.patch) is test-only and targets `bade9866278877923de0f247e95d7bd5d813b2b9`. Its SHA-256 is `51c7f84b519b4613f5a20b9c59c7d95bbe7100665a0da138f18b69ad81c31f13`; it contains the `inc2_control_` canonical-`Preserved` and sibling-symlink-alias runtime oracles. It was applicability-checked against a disposable extraction of that base but was neither applied nor run here, so no red observation is claimed.

## Inherited open items

- `Assessed(Refused)` still overstates a claim-authority construction failure; increment 3 owns the closed typed mapping.
- The Unix-only separator guard in `is_custody_record_name` remains intentionally unrepaired.
- The checked-scan entry-error loop remains non-latching.
- Boot sweeps now avoid up to fourteen unnecessary read-only exact-absence `git` observations; fewer subprocesses are expected, but no measurement is claimed.

## OPERATOR EVIDENCE — PENDING

- [ ] `cargo fmt --all -- --check` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (implementation point) — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (handoff point) — PENDING OPERATOR

## OPERATOR PROBE — PENDING

```text
git apply docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc2-genuine-red-control.patch
CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked inc2_control_ -- --nocapture
```

Apply the control only to the untouched base tree. Record its result separately; this candidate asserts only the frozen artifact and command.

## Final counted-line worksheet

Candidate counts are nonblank physical additions before the operator fmt gate; the operator must remeasure the formatted candidate before evidence handoff. Shared fixture lines are assigned to their first consuming component; no contingency or borrowing is used.

Rows C2-1, C2-2, and C2-3 sit exactly at their caps. They have no unclaimed slack: the operator's post-fmt remeasurement must apply the stated stop-and-handoff rule to any overage.

| Component | Estimate | Candidate lines | Cap |
| --- | ---: | ---: | ---: |
| C1-1 guards, admission, assessment | 70 | 90 | 100 |
| C1-2 row carrier and projection | 30 | 23 | 45 |
| C1 subtotal | 100 | 113 | 145 |
| C2-1 recording probe and real authority | 50 | 75 | 75 |
| C2-2 sixteen-population table | 80 | 115 | 115 |
| C2-3 preserved control | 40 | 60 | 60 |
| C2-4 sibling tests | 60 | 85 | 85 |
| C2-5 outside-root test | 35 | 55 | 55 |
| C2-6 precedence tests | 45 | 65 | 65 |
| C2-7 invalid-pair test | 30 | 44 | 45 |
| C2-8 amendment and mechanical reads | 15 | 24 | 25 |
| C2 subtotal | 355 | 523 | 525 |
| C1 + C2 | 455 | 636 | 670 |
| C3-1 frozen control | 60 | 59 | 95 |
| C3-2 handoff | 115 | 82 | 150 |
| C3 subtotal | 175 | 141 | 245 |
| Total | 630 | 777 | 915 |
