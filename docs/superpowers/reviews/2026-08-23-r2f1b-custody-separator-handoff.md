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
- SHA-256: `d1da3c9fdaad62922ff360c63246a1b8319656bcc9631982317ddd470557d3d7`.
- Logical mutation: revert the classifier's shared dual-separator terminal-segment extraction to slash-only extraction.
- The patch applies cleanly and alters production code in `is_custody_record_name` only.
- Designated red tests:
  - `custody_record_path_is_invisible_to_the_legacy_sidecar_scanner`
  - `custody_record_name_rejects_retirement_residue_across_separator_spellings`
  - `custody_record_name_rejects_empty_stem_across_separator_spellings`
- Applied control result: run against the FULL `bridge-worktree` suite, the red population is **four** —
  tests 1–3 plus `sweep::checked_scan::tests::checked_scan_classifier_preserves_full_path_precedence_and_boundaries`,
  which joined the guard set when its defect-encoding expectation was corrected. An earlier revision of this
  line said "exactly tests 1–3"; that figure came from running the named tests under filters and was wrong.
  A population claim derived from a filtered run is not a population claim.
- The non-divergence guard intentionally stays green under the frozen control because it covers only rows whose old and repaired classifications are identical.
- The control was reversed after the run and the candidate source was restored; `git apply --check` confirms it reapplies cleanly.

## Deferred sibling classifier follow-up

- The separator-neutral follow-up ledger covers three sibling full-display-path classifiers: repaired `is_custody_record_name`, plus deferred `is_staged_custody_residue` and `is_custody_retirement_residue`.
- This slice deliberately leaves the two storage-report residue classifiers unchanged; repairing them together needs a separately capped follow-up.

## Local verification note

- `cargo fmt --all` and `git diff --check` completed successfully.
- All four focused candidate tests passed with the local offline Cargo cache before the control run.
- `checked_scan_classifier_preserves_full_path_precedence_and_boundaries` passed with the backslash empty-terminal-stem display classified as `None`.

- [x] `cargo fmt --all -- --check` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **see operator evidence below**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **see operator evidence below**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **see operator evidence below**


---

# Operator evidence

Candidate `d932e5a1`, parent `origin/main` = `5e3d70b2`.

## Gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | **exit 0** |
| `clippy --workspace --all-targets --locked -- -D warnings` | **exit 0** |
| `cargo test -p bridge-worktree --locked --no-fail-fast` | **exit 0, 366 passed / 0 failed** |
| `cargo test --workspace --locked --no-fail-fast` | 11 distinct failures — **identical to base, zero candidate-only** |
| `validate --repo-hygiene` (both points) | **exit 0** |

Counted lines: **84** against the 260 cap.

## Frozen control — CORRECTION to the candidate's population claim

SHA-256 `d1da3c9fdaad62922ff360c63246a1b8319656bcc9631982317ddd470557d3d7` — matches the handoff exactly and
applies cleanly to `d932e5a1`.

The candidate's handoff states the red population "was exactly tests 1–3". **That is incomplete.** It was
derived by running the four *named* tests individually under filters, so a test outside that set was never
observed. The operator ran the **full `bridge-worktree` suite** under the control: **362 passed, 4 failed**.

| Reddened test | named in spec? |
|---|---|
| `custody_record_name_rejects_retirement_residue_across_separator_spellings` | yes |
| `custody_record_name_rejects_empty_stem_across_separator_spellings` | yes |
| `custody_record_path_is_invisible_to_the_legacy_sidecar_scanner` | yes |
| `sweep::checked_scan::tests::checked_scan_classifier_preserves_full_path_precedence_and_boundaries` | **no** |

**The fourth reddening is correct and desirable.** That characterization test previously asserted
`classify(r"dir\.custody.v1.json") == Some(Custody)` — it encoded the defect as expected behaviour. The
candidate flips it to `None`, so reverting production now trips it. It joined the guard set by being fixed.

The task's "exactly tests 1, 2 and 3" criterion is therefore **amended**: a control that trips more guards
is stronger evidence, not weaker. The task's own instruction to "stop and report the actual population" is
what made the gap visible, and is retained.

The measurement lesson generalises: a population claim derived from a filtered run is not a population
claim. This is the same failure mode as reading the first `test result:` line of a multi-binary run.

## What the fix does

A private `custody_record_terminal_segment(stem)` derives one terminal segment with `rsplit(['/', '\\'])`,
used by **both** the non-empty-target check and the `ChildNameV2` retirement-namespace parse. The separate
forward-slash-only trailing guard is **removed**, not supplemented — one separator decision, not two.

No `Path::file_name`, no `MAIN_SEPARATOR`, no platform-conditional code, and no change to `bridge-core`.

## Consequent change outside the named file, disclosed

`sweep/checked_scan.rs` updates one characterization expectation, described above. Disclosed in the
candidate's handoff. Not scope creep — it is the test that had locked the defect in.

## An unsatisfiable acceptance bullet in the operator's own spec

The spec required tests 2–4 to fail on the pre-change tree. **Test 4 cannot**: it is the non-divergence
guard, and the spec's own ground-truth table shows the ordinary, staging and dot-stem rows never diverged.
Both reviewers flagged it and scoped it correctly — a handoff sentence, not a code change, not grounds to
block. Recorded here as the operator's defect, not the implementation's.

## Provenance

This spec was folded from two independently authored specs given byte-identical input — `gpt-5.6-sol`
(effort xhigh, 454s) and `opencode-go/ox-alpha-free` (109s). Both verified their anchors; neither
hallucinated. Both independently required a **multi-test** reddening control without being asked — the very
criterion the operator got wrong in T3b slice 5B.


## Control re-cut (SMELL-3)

The original control reverted the call site and left `custody_record_terminal_segment` unreferenced. Under
`cargo test` that is invisible, but under `clippy -D warnings` it fails with
`error: function \`custody_record_terminal_segment\` is never used` — **before reaching any of the intended
red tests**, so the control would appear to "work" for the wrong reason.

Re-cut with the same single logical mutation (slash-only segmentation at the call site) plus a discard that
keeps the helper referenced. Verified on `origin/main`:

- `cargo test -p bridge-worktree` → **362 passed, 4 failed** — the same four-test red population as before.
- `clippy -p bridge-worktree --all-targets -- -D warnings` → **exit 0**, zero dead_code findings.

New SHA-256: `d1da3c9fdaad62922ff360c63246a1b8319656bcc9631982317ddd470557d3d7`.

Found by an independent reviewer (`opencode-go/ox-alpha-free`) during a head-to-head code-review comparison.
It was missed by the implement loop's two reviewers, by `gpt-5.6-sol`, and by the operator, all of whom
verified the control with `cargo test` only.
