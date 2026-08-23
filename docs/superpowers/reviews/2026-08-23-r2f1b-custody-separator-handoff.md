# Separator-neutral custody record classification handoff

## Candidate scope

- Base: `5e3d70b2`.
- Changed files:
  - `crates/bridge-worktree/src/custody.rs`
  - `crates/bridge-worktree/src/sweep/checked_scan.rs`
  - `docs/superpowers/reviews/2026-08-23-r2f1b-custody-separator-mutation-control.patch`
  - this handoff
- `Cargo.lock` and every manifest are untouched.

## Counted lines

- Task projection: 110 added nonblank physical Rust lines.
- Candidate total after `cargo fmt`: 84 added nonblank physical Rust lines against the base, below the 260-line cap.

## Frozen single-mutation control

- Path: `docs/superpowers/reviews/2026-08-23-r2f1b-custody-separator-mutation-control.patch`.
- SHA-256: `38b1fdc480766cf4a7becd11de5c4171c24e6968f81bb8ac094092c0c9a185ab`.
- Logical mutation: revert the classifier's shared dual-separator terminal-segment extraction to slash-only extraction.
- The patch applies cleanly and alters production code in `is_custody_record_name` only.
- Designated red tests:
  - `custody_record_path_is_invisible_to_the_legacy_sidecar_scanner`
  - `custody_record_name_rejects_retirement_residue_across_separator_spellings`
  - `custody_record_name_rejects_empty_stem_across_separator_spellings`
- Applied control result: tests 1–3 each exited 101 and failed on their required backslash row; test 4 exited 0, so the red population was exactly tests 1–3.
- The non-divergence guard intentionally stays green under the frozen control because it covers only rows whose old and repaired classifications are identical.
- The control was reversed after the run and the candidate source was restored; `git apply --check` confirms it reapplies cleanly.

## Deferred sibling classifier follow-up

- The separator-neutral follow-up ledger covers three sibling full-display-path classifiers: repaired `is_custody_record_name`, plus deferred `is_staged_custody_residue` and `is_custody_retirement_residue`.
- This slice deliberately leaves the two storage-report residue classifiers unchanged; repairing them together needs a separately capped follow-up.

## Local verification note

- `cargo fmt --all` and `git diff --check` completed successfully.
- All four focused candidate tests passed with the local offline Cargo cache before the control run.
- `checked_scan_classifier_preserves_full_path_precedence_and_boundaries` passed with the backslash empty-terminal-stem display classified as `None`.

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**
