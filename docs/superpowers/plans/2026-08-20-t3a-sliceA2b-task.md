---
task-type: implement
---

# A2b — return the exact-absence report

## Description

A2a landed on `main` in two halves: one private checked-scan engine serving both
worktree-sweep projections (#62), and ten characterization scenarios pinning what
that refactor preserves (#63). Throughout, `sweep_orphans_with_exact_absence`
kept returning `()` and the rich exact outcome was computed and discarded.

A2b cashes it in: change that function to return `ExactAbsenceSweepReportV1`, and
populate the report from the outcome the engine already produces.

**This is the first slice in this lane with genuine runtime red.** A1 landed the
report vocabulary and A2a landed the production behavior, so tests asserting a
returned report cannot compile — let alone pass — against the untouched base.
That is the point, and it changes what your evidence must prove; see
`### Genuine red and its control` below.

### What exists at your base

- `crates/bridge-worktree/src/sweep.rs` — `sweep_orphans_with_exact_absence`
  delegates to `sweep_orphans_with_exact_absence_with_pin_opener`, which returns
  a private `ExactScanOutcomeV1`. The public wrapper currently consumes that
  outcome through an accessor and returns `()`.
- `project_exact_scan_result` builds the outcome from the engine's
  `CheckedScanCompletedV1`, carrying rows, an iterator-error count, and a
  `RootObservationSetV1`.
- `crates/bridge-worktree/src/sweep/report.rs` — the A1 vocabulary, including
  `ExactAbsenceSweepReportV1` with private fields `requested_root`,
  `canonical_root`, `scan`, `entries`, and `pub(crate) fn new` constructors that
  still carry `#[allow(dead_code)]` because nothing has called them yet.
- `EXACT_ABSENCE_POLICY_READY_V1` is `false`, and `effective()` filters on it.

Read all of this before editing. Line counts are deliberately not stated here;
measure whatever you need yourself.

### Scope

A2b owns:

- the public return-type change on `sweep_orphans_with_exact_absence`;
- populating `ExactAbsenceSweepReportV1` from the exact outcome — requested root,
  canonical root, scan status, and one entry per retained row with its
  production-computed decision;
- consuming A1's four constructor `dead_code` allowances and removing every one
  that is no longer needed;
- keeping the five `sweep_orphans` boot callers in `bin/a2a-bridge/src/main.rs`
  compiling unchanged, since `sweep_orphans` calls the exact route in statement
  position;
- the four inherited findings below;
- the handoff.

A2b does **not**:

- set `EXACT_ABSENCE_POLICY_READY_V1` to true;
- add the increment-2 population-admission rule;
- populate real root observations — production still yields
  `RootObservationSetV1::default()`, so root classification stays `Unavailable`;
- add ownership, locking, deletion, or any action authority. **T3a decides and
  reports; T3b acts.** A later actor must re-open, re-read, re-bind, and re-prove
  exact absence under its own lock regardless of what this report says.
- repair the Unix-only separator guard in `is_custody_record_name` (see
  `### Inherited open items`).

---

## Genuine red and its control

Because this slice changes a public signature, a test that asserts a returned
report will not compile on the untouched base. A test that cannot compile proves
nothing about behavior on its own, so:

- classify each new test explicitly as **genuine runtime red**,
  **compiler-only return-shape evidence**, or **characterization**;
- for tests you classify as genuine runtime red, supply a reproducible control:
  freeze an exact test-only patch against a recorded base tree, record that
  tree's identity and the patch's content, and state the command that runs it.
  Do not claim red you did not observe;
- do not manufacture red. If a behavior was already proven by A2a's
  characterization tests, say so and classify accordingly.

The operator runs the gates and will ask for that control.

---

## Inherited findings

These four were raised in earlier review rounds and explicitly deferred to A2b.
Each must be resolved, not merely acknowledged.

### F4 — the reproducible red control

As above. A2b's red is real, so it needs a frozen base tree plus patch identity
rather than an assertion that the tests "would fail."

### F6 — the source-incompatible public break

`bridge-worktree` inherits workspace version `0.3.1` and no workspace member sets
`publish = false`, so it is publishable by default. Changing the return type from
`()` to `ExactAbsenceSweepReportV1` is source-incompatible for any caller that
constrains the result to unit — unit-returning function pointers, closures,
generic consumers, or unified expression branches.

A2a already settled the shape of this decision: **accept the break, add no
compatibility wrapper and no second entry point.** No external consumer is
established, and none may be asserted. What A2b owes is the record: state the
accepted break in the handoff and note that the release owner must not publish
the changed crate as patch-compatible with `0.3.1`. Version selection itself is
outside this slice but blocking before publication.

### F8 — birthtime-capability result visibility

`BirthTimeV1::from_metadata` is `metadata.created().ok()`, which errors on
filesystems without creation-time support, so a capability probe may legitimately
observe either `Some` or `None`. Ordinary captured `cargo test` output does not
reveal which occurred, making a passing test uninformative.

If A2b includes any birthtime-capability observation, make the observed branch
visible — a targeted `--nocapture` probe or a machine-readable artifact recording
the fixture identity, the observed capability, and the resulting classifier
expectation. If A2b includes no such observation, say so explicitly and defer.

### F9 — possible versus guaranteed resolver observations

`deepest_existing_path` is defined in `crates/bridge-core/src/fs_custody.rs` and
installed as the production resolver by `compare_path_identities`. A mutation
inventory that lists its final stability-bracket calls as unconditional is wrong:
the comparator can return `CannotProve` after an unavailable initial resolution
and before those calls are reached.

Distinguish possible call edges from observations guaranteed on every execution,
and document the early-return branches.

---

## Inherited open items — record, do not act

- **The Unix-only separator guard.** `is_custody_record_name` guards the
  empty-basename case with `!stem.ends_with('/')`. On a backslash-spelled path
  the guard does not fire and classification diverges from the Unix spelling.
  A2a-2 characterized this deliberately. A2b preserves it and carries the note
  forward; repairing it needs its own slice and its own review.

---

## Handoff and custody

Create `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-handoff.md`.

**You make the implementation-candidate commit only.** Gate execution and the
handoff-only evidence commit belong to the host operator: this container's egress
cannot fetch the pinned `a2a-lf` dependency, so `cargo` cannot build here. Do not
attempt the gates, do not run `git diff --cached --check`, and do not fabricate
totals. Reporting a gate as blocked is correct; inventing one is not.

Carry these six lines unticked under a `## OPERATOR EVIDENCE — PENDING` heading:

- [ ] `cargo fmt --all -- --check` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (implementation point) — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (handoff point) — PENDING OPERATOR

The handoff must record: base identity and clean-tree status; a pre-edit
checkpoint with each factual anchor's disposition and source location, the
per-row estimates, and the proceed-or-stop decision; every changed file; the
test-name-to-evidence-category table including the genuine-red control; the
accepted semver break; which `dead_code` allowances were consumed and which
remain, with a reason for each survivor; that production root observations remain
`Unavailable` and readiness remains `false`; the F9 possible-versus-guaranteed
distinction; the Unix-only-separator note carried forward; and the final
counted-line worksheet.

Do not consult a template outside the repository; none exists there and these
inline requirements are the owner-approved replacement.

## Sizing

Counted lines are added nonblank physical lines after the fmt gate, one row per
line, no contingency, no borrowing.

| Counted component | Estimate | Cap |
|---|---:|---:|
| `sweep.rs` return-type change and report population | 90 | 120 |
| `report.rs` allowance cleanup and any constructor adjustment | 30 | 50 |
| Report-population and return-shape tests | 200 | 260 |
| Genuine-red control: frozen patch, recorded identity, and its documentation | 40 | 70 |
| Interim A2b handoff | 90 | 110 |
| **Total** | **450** | **610** |

Per-test cost is measured at roughly 28 nonblank lines from the tests already in
this crate, not estimated. Re-measure against your base before editing. If a row
will exceed its cap, stop and report the revised estimate rather than compressing
tests or evidence.

## Acceptance Criteria

Gates are operator-owned, so these are the conditions you are responsible for.

1. `sweep_orphans_with_exact_absence` returns `ExactAbsenceSweepReportV1`; no
   compatibility wrapper and no second public entry point is added.
2. The report is populated from the production exact outcome: requested root as
   the caller spelled it, canonical root when canonicalization succeeded, the
   scan status, and one entry per retained row.
3. Each entry carries the production-computed decision — decisions are not
   recomputed in the report layer.
4. `EXACT_ABSENCE_POLICY_READY_V1` remains `false` and `effective()` is
   unchanged.
5. Production root observations remain `RootObservationSetV1::default()`, so root
   classification remains `Unavailable`.
6. All five `sweep_orphans` boot callers in `bin/a2a-bridge/src/main.rs` continue
   to compile with no source change, or every required change is named and
   justified.
7. A1 constructor `dead_code` allowances that are now exercised are removed; any
   allowance that remains has a stated reason.
8. Every new test is classified as genuine runtime red, compiler-only
   return-shape evidence, or characterization, and no test is misclassified as
   red.
9. The genuine-red control exists: a frozen test-only patch, the recorded base
   tree identity, and the command that runs it.
10. The accepted semver break is recorded, with the note that the crate must not
    be published as patch-compatible with `0.3.1`.
11. The F9 mutation inventory distinguishes possible call edges from guaranteed
    observations and documents the early-return branches.
12. The Unix-only separator divergence is carried forward as an open item and is
    not repaired here.
13. No new dependency is added; `Cargo.toml`, `Cargo.lock`, and
    `crates/bridge-worktree/Cargo.toml` are unchanged.
14. The handoff exists with the six `PENDING OPERATOR` lines unticked, and
    exactly one implementation-candidate commit exists.
15. Every counted worksheet row and the total remain within cap, or the run
    stopped and reported instead.

Do not claim any gate result. Do not tick a pending box.

## Files

- `crates/bridge-worktree/src/sweep.rs` — the return-type change and report
  population.
- `crates/bridge-worktree/src/sweep/report.rs` — allowance cleanup; the A1
  vocabulary is otherwise settled.
- `crates/bridge-worktree/src/sweep/checked_scan.rs` — read-only unless a test
  needs the shared harness extended.
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs` — the external
  consumer; the natural home for public return-shape evidence.
- `bin/a2a-bridge/src/main.rs` — read-only reference for the five boot callers;
  change only if mechanically required, and justify it.
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-handoff.md` —
  create.
- `Cargo.toml`, `Cargo.lock`, `crates/bridge-worktree/Cargo.toml` — must not
  change.

## Spec Refs

Authoritative at your base commit:

- `crates/bridge-worktree/src/sweep.rs`
- `crates/bridge-worktree/src/sweep/report.rs`
- `crates/bridge-worktree/src/sweep/checked_scan.rs`
- `crates/bridge-core/src/fs_custody.rs` — `deepest_existing_path`,
  `compare_path_identities`, `BirthTimeV1::from_metadata`
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2a-1-handoff.md`
  — names F4, F6, F8, F9 as deferred here
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2a-2-handoff.md`
  — the Unix-only separator open item

## Commit Message

feat(worktree): return the exact-absence sweep report

Change `sweep_orphans_with_exact_absence` from `()` to
`ExactAbsenceSweepReportV1` and populate the report from the exact outcome the
checked-scan engine already produces: requested root, canonical root, scan
status, and one entry per retained row carrying its production-computed
decision.

Consumes A1's constructor allowances. Readiness stays false, production root
observations stay unavailable, and the report retains ordered historical
evidence rather than authority — a later actor must re-prove exact absence under
its own lock.

Accepts a source-incompatible public break at workspace version 0.3.1; the crate
must not be published as patch-compatible.

## Falsification license

Every symbol, signature, caller count, and behavioral statement above is an
operator claim measured against your base. The repository is authoritative.

If the public signature differs; `project_exact_scan_result` does not carry rows,
an iterator-error count, and root observations; the report's fields or
constructors differ from those described; the readiness gate is not `false`; the
boot-caller count differs; or any inherited finding no longer applies, record the
exact repository evidence and stop before editing.

Finding the work smaller than described is a good outcome.
