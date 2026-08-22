---
task-type: implement
---

# T3b slice 2 — the re-prove gate

## Description

Turn a held settlement window plus a sweep report entry into a **proved subject**, or a typed refusal.
**This slice is still effect-free.** No transition, no rename, no unlink, no publication, no `git`, no
process spawn. It builds the boundary that makes the report's lack of authority executable.

Base: `origin/main` = `c65c8eca`.

### The boundary that does not move

> The report carries **ordered historical evidence, not authority**. A later actor must **re-open,
> re-read, re-bind, and re-prove** exact absence under its own lock, regardless of what the report says.

Slice 1 supplied the lock. This slice supplies the proof. Settlement itself is slice 4's; do not build it here.

### Falsification license

Every factual claim below is a tripwire. If any anchor is false — a symbol absent, a visibility different,
a signature other than stated — **stop and report it**. Do not adapt the design around a false anchor and
do not invent a replacement. A spec that misdescribes the tree is the operator's defect to fix, and this
lane has already lost dispatches to exactly that.

### Anchors, verified at `c65c8eca` by the operator

Re-read each before relying on it.

- `settle::SettlementWindowV1` exposes `open`, and the accessors `record`, `pinned_root`, `record_name`,
  `custody_id`, `worktree_path`. `settle::SettlementWindowRefusalV1` is its refusal type.
- `sweep::checked_scan::scan_compatibility_with_pin_opener` is declared `pub(super)`.
- `sweep::project_exact_scan_result` is a private free function in `sweep.rs`.
- `settle` and `sweep` are **sibling** `pub mod` declarations at the crate root in `lib.rs`.
- The report type in `sweep::report` exposes `has_authoritative_scan`.
- `UnusedCandidateDecisionV1::Authorized` is the admitting decision; the report's eligibility predicate
  additionally excludes `ExactAbsenceRecordAssessmentV1::Legacy`.
- `custody::LEGAL_CUSTODY_TRANSITIONS_V1` is frozen and must not change in this slice.

### The seam these anchors force — read this before designing

The plan for this slice named `checked_scan::scan_compatibility_with_pin_opener` and
`project_exact_scan_result` as the machinery to re-run. Because `settle` is a **sibling** of `sweep`, not a
child, `pub(super)` and private-to-`sweep` both put those symbols **out of scope for `settle`**. Calling
them from `settle.rs` will not compile.

Resolve this by adding **one narrow `pub(crate)` seam inside `sweep`** that scopes the existing scan to a
single enumerated record name and returns a typed outcome. `settle::reprove_under_window` calls that seam.

Two constraints on the seam, both load-bearing:

1. **It must drive the same code path the report drives.** Re-implementing the scan, the projection, or the
   decision inside `settle` would let the acting path drift from the reporting path — the precise failure
   this whole boundary exists to prevent. Route through the existing scan and projection.
2. **It must return a decision, not a report.** A report is historical evidence by construction; handing one
   back would reintroduce the authority confusion at a new layer.

Do not widen any other visibility. If the seam cannot be built without widening more, stop and report.

## What this slice builds

**`settle::reprove_under_window`** — takes a held `SettlementWindowV1` and the one report entry naming its
subject, and returns either a proved capability or a typed refusal. It must require **all** of:

- the report's `has_authoritative_scan` holds;
- the entry's decision is `Authorized` and its assessment is not `Legacy`;
- the record read under the window is **byte-identical** to the one the entry describes;
- the record's state is `ProtectionPrepared` with the claim population that state requires.

**`ProvenSettlementV1`** — a capability with **no public constructor**, reachable only through a successful
`reprove_under_window`. It **owns** the window it was proved under, so a proof cannot outlive its lock. It
exposes read-only access to the proved subject; it exposes nothing that mutates.

**A tri-state refusal type.** Distinguish *refused* (proved ineligible) from **`cannot-prove`** (the evidence
needed is unavailable). Both deny settlement; conflating them would let a later slice treat an unprovable
subject as a proved-negative one, and the stranded-marker residual in the plan depends on that distinction
being visible.

## Required tests

Each must document the production mutation it catches.

1. `a_stale_report_is_never_authority_the_window_reproves` — recreate the target **after** the report is
   taken but **before** the window opens. The gate must refuse, and the custody record's bytes must be
   unchanged by the attempt.
2. `a_record_replaced_between_report_and_window_refuses` — replace the record with a different decodable
   record between report and window; the gate must refuse rather than prove against the new bytes.
3. A `cannot-prove` case, asserting the tri-state arm specifically — not merely that it did not prove.
4. A non-authoritative scan refuses, even with an otherwise-`Authorized` entry.
5. A `Legacy` assessment refuses.
6. A wrong-state record refuses — a state other than `ProtectionPrepared`, or `ProtectionPrepared` without
   its required claim population.
7. `the_proof_cannot_outlive_its_window` — a compile-fail or ownership assertion showing
   `ProvenSettlementV1` cannot be held past the window's release.
8. A bounded no-effect audit over the added production path: it may reach lock acquisition, directory
   pinning, descriptor-relative regular-file reads, canonical decoding, the scan seam, allocation and
   tracing, and must have **no** edge to rename, unlink, publication, transition, settlement, provider
   removal, prune or process spawn.

## Size

Projection **520** counted lines against a cap of **770**. Counted lines are added nonblank physical lines
after `cargo fmt`, Rust only. Note that a `grep` for added nonblank lines already excludes blanks — do not
subtract them a second time.

If the projection is going to exceed the cap, stop before editing and report the revised estimate. Do not
delete required tests to fit.

## Frozen control — a mutation control, and why not a behavioural red

On `c65c8eca` no symbol this slice adds exists, so any control naming `reprove_under_window` is a **compile
error**, and this lane has root-caused compile-error "reds" as non-evidence. Freeze instead a
**single-mutation control against this slice's own head**:

- Path: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice2-mutation-control.patch`
- One logical mutation, chosen so that removing it defeats the re-prove rule — for example, accept the
  report entry's decision without re-reading the record under the window.
- It must redden **exactly one** named test and no other.
- Record its SHA-256 in the handoff.

```text
git apply docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice2-mutation-control.patch
CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked settle:: -- --nocapture
```

## Handoff

Create `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice2-handoff.md` recording the base, the changed-file
list, the counted line total against the 770 cap, the bounded effect audit, and the frozen control's path,
SHA-256, mutation description and single reddening test.

**Do not record this candidate's own head commit or tree sha in the handoff.** The review loop amends the
candidate on each attempt, so any head sha written inside the handoff is rewritten by the next amend and
becomes unreachable. That binding is the **operator's**, recorded in the evidence commit after the candidate
is final. Slice 1 lost two of its three review rounds to this; it is a spec defect that has now been fixed.

End the handoff with exactly these six unticked lines:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

The base control is `bridge-worktree` **338 passed** at `c65c8eca`. The operator runs the gates, verifies the
patch hash, runs the mutation control against the recorded head, and commits only the completed handoff.

## Acceptance criteria

- [ ] `reprove_under_window` requires all four conditions above; each has a test that fails without it.
- [ ] `ProvenSettlementV1` has no public constructor and owns its window.
- [ ] The refusal type distinguishes refused from cannot-prove, and a test asserts the cannot-prove arm.
- [ ] Exactly one new `pub(crate)` seam is added inside `sweep`; no other visibility widens.
- [ ] The re-prove routes through the existing scan and projection rather than reimplementing either.
- [ ] `LEGAL_CUSTODY_TRANSITIONS_V1` is unchanged.
- [ ] The added path has no edge to rename, unlink, publication, transition, settlement, provider removal,
      prune or process spawn, and the no-effect test freezes that audit.
- [ ] Counted lines stay at or under 770.
- [ ] The frozen control exists at the named path, is SHA-256-recorded in the handoff, and names exactly one
      test that must redden.
- [ ] The handoff records no head commit or tree sha for this candidate.
- [ ] `Cargo.lock` and every manifest are untouched.
