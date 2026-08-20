---
task-type: implement
---

# A2a-2 — characterization evidence for the checked-scan engine

## Description

A2a-1 landed on `main`: both worktree-sweep projections now route through one
private checked-scan engine, and its correctness behavior — the decision matrix
for both record kinds, unreadable-custody-with-zero-probe, canonical-root and
unit-return preservation, and public-signature pinning — is proven.

This slice adds the **characterization** evidence that was explicitly deferred:
ten scenarios that pin what the refactor preserves, as distinct from what it
must decide.

**This is a tests-only slice.** Do not modify production source. If a test
cannot be written without a production change, stop and report under the
falsification license rather than making the change.

### What exists at your base

`crates/bridge-worktree/src/sweep/checked_scan.rs` contains the engine, the
compatibility source and session, and a `#[cfg(test)] mod tests` with a shared
harness you should reuse rather than rebuild:

- `Script` — an injected `CheckedScanSourceV1` / `CheckedScanRootSessionV1` with
  a scripted name stream, so enumeration order and iterator errors are
  deterministic;
- `Log` / `note` — an operation log for asserting call sequences;
- `sidecar(source, worktree)` — builds a legacy `WorktreeSidecar`;
- `decoded_custody()` — builds a valid `WorktreeCustodyRecordV1`;
- `temp_root(label)` — a scratch directory path.

Available to tests through the seam:

- `CheckedScanRowV1::parts()` and `::record_path()`;
- `CheckedScanCompletedV1::into_action_rows()` and `::into_exact_parts()`;
- `scan_compatibility_with_pin_opener`.

Read these before writing. Where the harness needs a small extension, extend it;
do not fork a parallel one.

**Prefer the injected `Script` source over real directories.** Enumeration order
from a real `read_dir` is unspecified, so ordering assertions across two
independent traversals are not a sound oracle. Scripted streams make order,
iterator errors, and refusals deterministic. Use a real directory only where the
scenario is genuinely about filesystem behavior, and say so in the handoff.

---

## The ten scenarios

Write one test per named scenario, with these exact names.

### Selection and classification

1. `checked_scan_classifier_preserves_full_path_precedence_and_boundaries`

   The classifier receives the full lossy joined display path and applies
   legacy-suffix first, custody second, delegating custody classification to
   `is_custody_record_name`. Cover the exact `.custody.v1.json` basename,
   `dir/.custody.v1.json`, and the backslash spelling `dir\.custody.v1.json`.

   `is_custody_record_name` strips the suffix and rejects an empty stem or a
   stem ending in `'/'` — a Unix-only separator test. On a backslash-spelled
   path the stem ends in `'\'`, so the guard does not fire and the classification
   differs from the Unix spelling. **Characterize that divergence; do not repair
   it.** A2a preserves behavior. Record it in the handoff as a latent
   platform-dependency worth a future decision.

2. `checked_scan_silently_omits_bad_legacy_and_retains_bad_custody`

   A malformed legacy sidecar is silently omitted — `read_sidecar` is
   `std::fs::read(...).ok()?` then `serde_json::from_slice(...).ok()`, so both
   failures yield `None` with no diagnostic. An unreadable custody record is
   **retained** as `UnreadableCustody` with its refusal preserved. Prove the
   asymmetry in one test.

### Traversal accounting

3. `checked_scan_counts_iterator_errors_and_continues_in_injected_order`

   A scripted stream mixing `Ok` names and `Err` items: each error increments
   the count and traversal continues; successful names are processed in the
   scripted order. Assert the exact final count and the exact order.

### Root observations

4. `nondefault_root_observations_survive_exact_without_changing_rows_or_decisions`

   Supply non-default root observations through the injected source and assert
   they reach `into_exact_parts` unchanged, while rows and decisions are
   byte-for-byte what the default-observation run produces.

   Production always returns `RootObservationSetV1::default()`, so this is
   necessarily an injected-seam test. Say so in its evidence classification
   rather than implying production populates observations.

5. `enumeration_refusal_retains_canonical_root_and_skips_assessment`

   On `CheckedScanOpenRefusalV1::CannotEnumerate`, the already-observed
   canonical root is retained in the exact outcome and **no** assessment,
   probe call, or decision event occurs.

### Projection shape

6. `action_projection_erases_only_action_metadata`

   `into_action_rows` yields exactly `Vec<(String, ScannedWorktreeRecordV1)>`,
   discarding exact names, iterator-error count, and root observations — while
   the same completed result via `into_exact_parts` retains all three. One
   engine result, two projections, asymmetric erasure.

7. `injected_sources_use_production_action_and_exact_projections`

   The injected cases traverse the **real** production projections, not
   duplicated test logic. Assert against the production entry points rather than
   re-implementing projection behavior in the test.

8. `injected_sources_prove_action_and_exact_projection_equivalence`

   For the same scripted stream and identical opener outcomes, both projections
   agree on selected display paths and their order, on valid legacy sidecars by
   structural equality, on decoded custody records by structural equality, and
   on unreadable-custody refusals by exact equality.

   Do **not** assert equality for the three exact-only details — retained
   `OsString` names, iterator-error count, root observations. They have no action
   counterpart; claiming equivalence there would be false.

### Pin-failure seams

9. `report_side_pin_failure_uses_post_canonicalization_opener_seam`

   Deterministic pin failure on the report route is injected **below** supplied-
   root canonicalization: canonicalization still happens, then pin opening
   fails. Legacy reads still succeed; custody rows refuse as not-pinnable.

10. `compatibility_pin_failure_preserves_legacy_and_refuses_custody`

    The same deterministic pin failure through the action projection, proving
    both projections share the behavior rather than one asserting it by
    construction.

---

## Evidence classification

Every test here is **characterization or new-seam mechanism evidence**. None is
genuine runtime red: A2a-1 already landed the production behavior, so these
tests pass on the untouched base by construction.

Do not claim runtime red. For each test, document the production or
evidence-infrastructure mutation it would catch, and name its category.

---

## Handoff and custody

Create `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2a-2-handoff.md`.

**You make the implementation-candidate commit only.** Gate execution and the
handoff-only evidence commit belong to the host operator, because this
container's egress cannot fetch the pinned `a2a-lf` dependency and therefore
cannot build. Do not attempt the gates. Do not run `git diff --cached --check`.
Do not fabricate totals.

Carry these six lines unticked under a `## OPERATOR EVIDENCE — PENDING` heading:

- [ ] `cargo fmt --all -- --check` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (implementation point) — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (handoff point) — PENDING OPERATOR

The handoff must record: the exact base identity and clean-tree status; a
pre-edit checkpoint with each factual anchor's disposition and source location,
the per-row estimates, and the proceed-or-stop decision; every changed file; the
test-name-to-evidence-category table; which scenarios used the injected source
versus a real directory and why; the backslash-classifier divergence as
characterized-not-repaired; that production root observations remain
`Unavailable` and readiness remains `false`; and the final counted-line
worksheet.

Do not consult a template outside the repository; none exists there and the
inline requirements above are the owner-approved replacement.

## Sizing

Counted lines are added nonblank physical lines after the fmt gate, one row per
line, no contingency, no borrowing.

| Counted component | Estimate | Cap |
|---|---:|---:|
| Ten characterization tests | 280 | 340 |
| Shared harness extensions | 40 | 70 |
| Interim A2a-2 handoff | 80 | 100 |
| **Total** | **400** | **510** |

The per-test figure derives from the **measured** cost of the tests already in
this file — about 28 nonblank lines each — rather than a guess. Re-measure
against your base before editing. If a row will exceed its cap, stop and report
the revised estimate rather than compressing tests or evidence.

## Acceptance Criteria

Gates are operator-owned, so these are the conditions you are responsible for.

1. All ten tests exist with the exact names given above, one test per named
   scenario.
2. Each test exercises the real production projections; none re-implements
   projection logic inside the test.
3. The classifier test covers the exact `.custody.v1.json` basename,
   `dir/.custody.v1.json`, and the backslash spelling, and characterizes the
   Unix-only-separator divergence without repairing it.
4. The omission test proves the asymmetry: malformed legacy silently omitted,
   unreadable custody retained with its refusal.
5. The iterator-error test asserts both the exact final count and the exact
   processing order from a scripted stream.
6. The non-default-observation test is classified as injected-seam evidence and
   does not imply production populates observations.
7. The enumeration-refusal test proves the canonical root is retained and that
   no assessment, probe call, or decision event occurs.
8. The equivalence test asserts agreement on display paths and order, legacy
   sidecars, decoded custody records, and unreadable-custody refusals — and does
   **not** assert equality for retained `OsString` names, iterator-error count,
   or root observations.
9. Both pin-failure tests inject below canonicalization and prove the behavior
   on both projections.
10. No production source file is modified, and no existing Rust is reformatted.
11. Every test documents the production or evidence-infrastructure mutation it
    catches and names its category; none is labeled genuine runtime red.
12. The handoff exists at the named path and carries the six
    `PENDING OPERATOR` lines unticked.
13. Exactly one implementation-candidate commit exists; no evidence commit is
    attempted.
14. Every counted worksheet row and the total remain within cap, or the run
    stopped and reported instead.

Do not claim any gate result. Do not tick a pending box.

## Falsification license

Every symbol, behavior, and file claim above is an operator claim measured
against your base. The repository is authoritative. If the harness differs, an
accessor is absent, `is_custody_record_name` does not behave as described,
`read_sidecar` does not silently omit failures, the projections do not have the
stated asymmetry, or any scenario is unwritable without a production change,
record the exact source evidence and stop before editing.

Finding the work smaller than described is a good outcome.

## Files

- `crates/bridge-worktree/src/sweep/checked_scan.rs`
  - add tests to the existing `#[cfg(test)] mod tests`; extend the shared
    harness where needed;
  - **do not modify production code above `mod tests`.**
- `crates/bridge-worktree/src/sweep.rs`
  - read-only reference for the production projections and routing.
- `crates/bridge-worktree/src/custody.rs`
  - read-only reference for `is_custody_record_name` and custody decode
    refusals.
- `crates/bridge-worktree/src/provider_path.rs`
  - read-only reference for `read_sidecar` and `canonicalize_lenient`.
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2a-2-handoff.md`
  - create.
- `Cargo.toml`, `Cargo.lock`, `crates/bridge-worktree/Cargo.toml`
  - must not change; this slice adds no dependency.

## Spec Refs

Authoritative at your base commit:

- `crates/bridge-worktree/src/sweep/checked_scan.rs`
- `crates/bridge-worktree/src/sweep.rs`
- `crates/bridge-worktree/src/custody.rs`
- `crates/bridge-worktree/src/provider_path.rs`
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2a-1-handoff.md`
  — the A2a-1 handoff, which names these ten scenarios as deferred here.

## Commit Message

test(worktree): characterize the checked-scan engine's preserved behavior

Add the ten characterization scenarios deferred from A2a-1: classifier
precedence and boundaries including the Unix-only separator divergence,
malformed-legacy omission against unreadable-custody retention, iterator-error
accounting and order, non-default root observations through the injected seam,
enumeration-refusal canonical-root retention, action-metadata erasure, and
projection equivalence with both pin-failure seams.

Tests only. No production source changes, and no new dependency.
