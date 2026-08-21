# A2b handoff — return the exact-absence report

## Scope and custody

- Implementation base: `a8b13fe685c9e261106498b9d3237da7d295a6ca`.
- Pre-edit `git status --short` was empty; no user change was present.
- This is the implementation candidate only. The host operator owns every gate and the later handoff-only evidence commit.
- No dependency, manifest, lockfile, policy-readiness, action-authority, locking, ownership, deletion, or root-observation population change is included.

## Pre-edit checkpoint

| Factual anchor | Disposition and source location |
| --- | --- |
| Public shape | Present: `sweep_orphans_with_exact_absence` returned `()` after consuming its private outcome in `crates/bridge-worktree/src/sweep.rs`. |
| Retained outcome | Present: `ExactScanOutcomeV1` and `project_exact_scan_result` retained canonical root, iterator errors, root observations, checked rows, and decisions in `sweep.rs`. |
| Engine projection | Present: `CheckedScanCompletedV1::into_exact_parts` exposed rows, error count, and `RootObservationSetV1` to its parent in `sweep/checked_scan.rs`. |
| A1 vocabulary | Present: `ExactAbsenceSweepReportV1` and its four `#[allow(dead_code)]` constructors were in `sweep/report.rs`. |
| Readiness | Present: `EXACT_ABSENCE_POLICY_READY_V1 = false`; `effective()` filters on that unchanged constant in `sweep/report.rs`. |
| Boot callers | Present: exactly five statement-position `sweep_orphans` calls at `bin/a2a-bridge/src/main.rs:3526,3897,4522,8206,9891`; that file is unchanged. |
| Deferred findings | Present: A2a-1 records F4/F6/F8/F9 and A2a-2 records the Unix-only separator divergence. |
| Decision | Proceed: every implementation anchor matched the task; the public API incompatibility is intentional. |

## Changed files

- `crates/bridge-worktree/src/sweep.rs` returns and projects the stored exact outcome, with no decision recomputation; its report tests cover canonicalization refusal and all root-observation classifier outcomes. The legacy `sweep_orphans` caller explicitly discards that report so its five boot callers remain source-unchanged.
- `crates/bridge-worktree/src/sweep/report.rs` consumes all four A1 constructor allowances.
- `crates/bridge-worktree/src/sweep/checked_scan.rs` removes six root-capture allowances exercised by the report projection, mechanically updates its inherited unit-return assertion, and forces iterator errors through the report projection.
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs` pins the public report return type.
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-genuine-red-control.patch` is the frozen base control.
- This handoff is `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-handoff.md`.

## Report projection and tests

| Test | Evidence category | What it establishes |
| --- | --- | --- |
| `exact_absence_sweep_reports_the_stored_runtime_decision` | genuine runtime red | Caller spelling, canonical root, complete/unavailable status, all retained row classes, stored decisions, and inactive `effective()`. |
| `exact_absence_sweep_reports_cannot_canonicalize` | characterization | The public report preserves the caller spelling, no canonical root, `CannotCanonicalize`, `Unavailable`, and no entries. |
| `exact_projection_reports_forced_iterator_errors` | characterization | An injected checked-scan fault reaches the public report as `Incomplete { skipped_entries: 2 }`. |
| `root_observation_classifier_reports_pinned_captures` | characterization | Three complete equal captures classify as `Pinned`. |
| `root_observation_classifier_reports_identity_changes_including_birthtime` | characterization | A changed inode or present-versus-absent birthtime classifies as `IdentityChanged`. |
| `root_observation_classifier_refuses_incomplete_captures` | characterization | A capture missing object identity remains `Unavailable`. |
| `public_scan_functions_keep_visibility_and_exact_signatures` | compiler-only return-shape evidence | The external consumer requires `fn(&str, &dyn ExactAbsenceProbeV1) -> ExactAbsenceSweepReportV1`. |
| `exact_route_preserves_canonical_scan_root_and_report_return` | compiler-only return-shape evidence | The inherited unit test keeps its canonical-root assertion and now constrains the wrapper to the report. |
| A2a/A2a-2 checked-scan scenarios | characterization | They already proved the engine's row, decision, iterator, and root-observation behavior; A2b does not relabel them as red. |

The report-projection test is a genuine runtime red: after the new public return is available it executes the production wrapper and checks the returned report. Its frozen base-tree control carries a corresponding report-projection runtime oracle. On the untouched base the call cannot compile because the public return is `()`, which is recorded as the base control's compiler barrier rather than misreported as a runtime observation.

### F4 — frozen base control

- Base tree: `a8b13fe685c9e261106498b9d3237da7d295a6ca`.
- Patch: `2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-genuine-red-control.patch`, SHA-256 `077abded3e9397115b4a59c9a4d2a5e305601c92881d90cfbe3c7a591fae779a`.
- It adds the focused `exact_absence_sweep_reports_the_stored_runtime_decision` test to the recorded base's existing unit-test module. The test directly exercises requested/canonical root reporting, complete/unavailable scan status, the retained legacy row, and its stored `Authorized` decision.
- Reproduction command, for the host operator: `git worktree add --detach /tmp/a2b-red-control a8b13fe685c9e261106498b9d3237da7d295a6ca && git -C /tmp/a2b-red-control apply /absolute/path/to/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-genuine-red-control.patch && CARGO_INCREMENTAL=0 cargo test --manifest-path /tmp/a2b-red-control/Cargo.toml -p bridge-worktree exact_absence_sweep_reports_the_stored_runtime_decision --locked`.
- The raw base stops at the intentional return-shape compiler barrier; once the candidate implementation supplies the return, this same oracle is the runtime report-projection assertion. Neither control was run in this container.

## Deferred and inherited findings

- F6 is accepted: changing the publishable `bridge-worktree` public return from `()` is source-incompatible. No compatibility wrapper or second public entry point is added. The release owner must not publish this change as patch-compatible with workspace version `0.3.1`; version selection blocks publication and is outside A2b.
- F8 is deferred explicitly: no birthtime-capability observation was added, so no `Some`/ `None` branch is claimed.
- F9 inventory: `compare_path_identities_with_resolver` can call the resolver for the two initial paths and, only after both initial resolutions succeed, for the two final stability-bracket checks through `ancestors_are_stable_with_resolver`. The final calls are possible, not unconditional. Byte-identical paths return `Same` before any resolver call; either unavailable initial resolution returns `CannotProve` before the final pair; unavailable or changed final observations also return `CannotProve`.
- The Unix-only separator divergence remains open: `is_custody_record_name` guards only `stem.ends_with('/')`, so `dir\\.custody.v1.json` intentionally differs from `dir/.custody.v1.json`. A2b does not repair it.

## Preserved safety boundaries

- `EXACT_ABSENCE_POLICY_READY_V1` remains `false` and `effective()` is unchanged.
- Production `CompatibilityCheckedScanRootSessionV1::finish` still returns `RootObservationSetV1::default()`, so production root classification is `Unavailable`.
- The synthetic root classifier is intentionally strict: equality includes `dev`, `ino`, and the optional birthtime, so a present-versus-absent birthtime is `IdentityChanged`. This conservatively preserves a possible capability disparity or inode-reuse signal; future real-observation population must reconsider this policy explicitly.
- The report reuses the exact projection's stored `UnusedCandidateDecisionV1`; it does not probe or re-decide in the report layer.
- The report is historical evidence only. A future actor must re-open, re-read, re-bind, re-admit, and re-prove exact absence under its own lock before any effect.

## Allowances and unchanged manifests

- Removed: the four A1 `dead_code` allowances on `ExactAbsenceSweepReportV1::new`, `ExactAbsenceScanStatusV1::new`, `ExactAbsenceSweepEntryV1::new`, and `CustodyRecordAssessmentV1::new`.
- Removed: the six field-scoped `dead_code` allowances on `RootIdentityCaptureV1` and `RootObservationSetV1` in `checked_scan.rs`; the production report projection now compares those captures. No A1/A2a allowance remains.
- `Cargo.toml`, `Cargo.lock`, and `crates/bridge-worktree/Cargo.toml` are unchanged.

## OPERATOR EVIDENCE — PENDING

- [ ] `cargo fmt --all -- --check` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (implementation point) — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (handoff point) — PENDING OPERATOR

## Counted-line worksheet

| Component | Added nonblank lines | Cap |
| --- | ---: | ---: |
| `sweep.rs` return and report projection | 96 | 120 |
| `report.rs` allowance cleanup | 0 | 50 |
| Report-population and return-shape tests | 197 | 260 |
| Frozen red control and documentation | 62 | 70 |
| Interim A2b handoff | 75 | 110 |
| Total | 430 | 610 |

These are the current candidate’s nonblank physical additions measured against the recorded base; the frozen control is counted directly after its whitespace cleanup. All operator gates above remain pending; the host operator must remeasure if a later gate changes them.
