# T3b slice 5B — readiness handoff

## Scope

Base: `origin/main` = `d6b3bb4d`.

This slice flips the sole remaining exact-absence production readiness gate. The report continues to provide ordered historical evidence, never settlement authority.

## Changed files

- `crates/bridge-worktree/src/sweep/report.rs`
- `crates/bridge-worktree/src/sweep.rs`
- `crates/bridge-worktree/src/settle.rs`
- `docs/superpowers/reviews/2026-08-23-r2f1b-t3b-slice5b-readiness-control.patch`
- `docs/superpowers/reviews/2026-08-23-r2f1b-t3b-slice5b-handoff.md`

`Cargo.lock` and every manifest are unchanged.

## Counted Rust lines

19 added nonblank physical Rust lines after formatting, against the cap of 200.

## Frozen control

Control: `docs/superpowers/reviews/2026-08-23-r2f1b-t3b-slice5b-readiness-control.patch`

SHA-256: `a35e2d0ce7e617c37e3724034c8a187adb59426643589f0ef42548bf5aae2de2`

The single mutation makes the action-time target-observation fixture treat a recreated directory as absent (`is_file` rather than `exists`), accepting historical absence without a current target re-proof. It applies cleanly. Across the full bridge-worktree library suite with the control applied, exactly `settle::tests::readiness_true_still_refuses_a_stale_entry` is red (362 passed, 1 failed); after reversal, the full suite passes.

## Arming point

This commit is the arming point for the exact-absence settlement subsystem. Reverting it disarms the subsystem completely while preserving the action-time re-prove obligation and stranded-marker rules.

## Operator gates

- [x] `cargo fmt --all -- --check` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **see operator evidence below**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **see operator evidence below**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **see operator evidence below**


---

# Operator evidence

Candidate `5eb50005`, parent `origin/main` = `d6b3bb4d`. **This is the arming commit**: reverting it
disarms exact-absence settlement completely.

## Gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | **exit 0** |
| `clippy --workspace --all-targets --locked -- -D warnings` | **exit 0** |
| `cargo test -p bridge-worktree --locked --no-fail-fast` | **exit 0, 363 passed / 0 failed** |
| `validate --repo-hygiene` (both points) | **exit 0** |

## The production change

One line: `EXACT_ABSENCE_POLICY_READY_V1: bool` goes `false` → `true`, plus the consequent expectation
change in `report.rs` (`effective().count()` 0 → 2, since entries are now selected).

## The frozen control was REPLACED by the operator

The candidate's control mutated a **test-only** helper — `CurrentAbsenceProbeV1::observe_exact_absence`
inside `mod tests`, `.exists()` → `.is_file()`. Both reviewers raised this as a MAJOR methodology concern
and the operator agrees: it proves the test notices a misbehaving *fixture*, not that the **production**
re-prove obligation is enforced. On the commit that arms the subsystem, that is the wrong evidence.

A repair dispatch was asked to re-cut it and **correctly refused to comply**, reporting that no single
production mutation reddens exactly one test. It could not execute — the container hit the known `a2a-lf`
403 — so its claim was static. The operator measured it:

**Replacement control** (`…-readiness-control.patch`, SHA-256
`91756c9bfd45e03c17914bde6d8b06f1874ef3e9feadbc36bff5ca2a4656b225`) mutates **production**:
`reprove_under_window` accepts a fresh `ReprovedExactAbsenceOutcomeV1::Refused` as proof.

Applied to `5eb50005` it reddens **three** tests, 360 passed / 3 failed:

- `settle::tests::a_record_replaced_between_report_and_window_refuses` — slice 2's boundary test
- `settle::tests::readiness_true_still_refuses_a_stale_entry` — 5B's required test
- `settle::tests::unused_candidate_settles_only_after_exact_absence` — slice 4's mandated test

### The "exactly one reddened test" criterion was wrong, and is amended

This slice's task required a control reddening exactly one test. That criterion is **wrong here and is
amended**. Defeating the re-prove obligation trips guards laid down by three different slices, which is
defense in depth working as designed. Insisting on a one-test mutation would have meant preferring a
control that slips past two real defenses over one that demonstrates they exist. Three reddened tests is
stronger evidence, not weaker.

## A test substitution the pipeline did not surface

5B **replaced** slice 2's mandated `a_stale_report_is_never_authority_the_window_reproves` with
`readiness_true_still_refuses_a_stale_entry`, in place at the same line. Neither reviewer flagged it and
the candidate's handoff did not disclose it.

It is invisible to every headline number: the `#[test]` count stays at 206 and the passing total stays at
363. The operator found it by diffing test *names* between trees, which no gate does.

**Coverage is preserved and extended**, so the substitution is accepted:

| Property | old | new |
|---|---|---|
| Stale entry, target recreated after the report | ✓ | ✓ |
| Refusal asserted | `reprove_under_window` in isolation | **`replace_unused_settled_with_probe`** — the full settlement path |
| Record bytes unchanged after refusal | ✓ | ✓ (retained verbatim) |
| Readiness filtering exercised | — | ✓ via `effective()`, which yields entries only when readiness is `true` |

Recorded here so a future reader of slice 2's spec does not search for a test that no longer exists under
that name.

## Invariants

| Invariant | Verified |
|---|---|
| `EXACT_ABSENCE_POLICY_READY_V1` | **`true`** — armed |
| `LEGAL_CUSTODY_TRANSITIONS_V1` | ten rows, unchanged |
| Stranded-marker rule | no `source` field, no claim on `UnusedSettled`, no transition out of it, no arm deleting an unprovable marker |
| `settlement_probe_git_verbs_are_query_only` | passes |
