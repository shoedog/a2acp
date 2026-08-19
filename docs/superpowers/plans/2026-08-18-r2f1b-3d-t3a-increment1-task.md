---
task-type: implement
---

# R2f1b 3d T3a increment 1 — a total typed report, and a truthful scan status

## Description

First of three increments rebuilding T3a's residual. Base: `main` = `9aedf175`.

**This increment is deliberately behavior-preserving.** It changes no decision, no
admission rule, and no refusal. It lands a typed reporting seam and a truthful scan
status, and nothing else. Read the whole of "What this increment is for" before
judging that as too small — landing it alone is the point.

**T3a DECIDES; T3b ACTS.** No path here writes, renames, or unlinks anything, and
no custody state, legal transition, record publication, settlement, deletion, or
CLI call-site behavior changes.

### What this increment is for

Two structural facts block the rest of T3a, and this increment exists to remove
both.

**F1 — the production path cannot carry a typed result.**
`sweep_orphans_with_exact_absence` returns `()` and merely
`tracing::info!(record, ?decision, …)`s each outcome. Any later test that wants to
assert an exact typed assessment *through the real production traversal* cannot,
because the traversal keeps nothing. A typed report has to exist first.

**F2 — new vocabulary cannot be behaviorally red in the increment that introduces
it.** A test naming a type that does not yet exist does not *fail* on the base — it
fails to **compile**, and a compile error is inadmissible as behavioral evidence.
So the vocabulary must land *before* the increment that changes behavior, or that
increment can never show a genuine red.

Hence the ordering: **vocabulary first (this increment, no behavioral red
available), behavioral change second (increment 2, with real red against this
one).**

### Consequence you must not "fix"

This increment defines closed enums whose arms it **never constructs** — every
readable custody record still reaches `Assessed`, exactly as today. That is
correct and deliberate: increment 2 populates `IneligiblePopulation` and the
guard-related `CannotConstructSubject` arms, and its tests can only be
behaviorally red if the vocabulary already compiles on its base.

Do **not** remove the unconstructed arms, do not gate them behind `cfg(test)`, and
do not add a placeholder that constructs one just to make it look used. Put a
comment on each saying which increment populates it. If `-D warnings` objects to
anything here, gate it the narrowest way that keeps it in the public API and say
what you did.

### Ownership is out of scope, by owner decision

T3a exposes **no** ownership input and **no** ownership variants. Ownership defers
wholly to T3b, which must consult active flights (including `TransferPublishing`)
and transferred recovery flights, reread durable evidence, and re-prove exact
absence under its action lock.

The reason is structural: all five production boot sweeps run at the top of
command entry points (`implement_cmd`, `implement_resume_cmd`, `run_workflow_cmd`,
`mcp_cmd`, `main`), whereas `WorktreeBackend` is constructed inside a per-run
session factory. T3a therefore cannot truthfully observe in-memory ownership and
must not pretend to. **Do not add `LocallyOwned`, `OwnershipCannotProve`, a
`recovery_owned` parameter, or any ownership plumbing.**

## What to build

All in `crates/bridge-worktree/src/sweep.rs`.

### 1. The report vocabulary

```rust
#[must_use]
pub struct ExactAbsenceSweepReportV1 {
    requested_root: String,
    canonical_root: Option<String>,
    scan: ExactAbsenceScanStatusV1,
    entries: Vec<ExactAbsenceSweepEntryV1>,
}

pub struct ExactAbsenceScanStatusV1 {
    enumeration: ExactAbsenceEnumerationV1,
    custody_root: CustodyRootObservationV1,
}

pub enum ExactAbsenceEnumerationV1 {
    Complete,
    Incomplete { skipped_entries: usize },
    Refused(ExactAbsenceRootRefusalV1),
}

pub enum ExactAbsenceRootRefusalV1 {
    CannotCanonicalize,
    CannotEnumerate,
}

pub enum CustodyRootObservationV1 {
    Pinned,
    Unavailable,
    IdentityChanged,
}

pub struct ExactAbsenceSweepEntryV1 {
    record_path: String,
    assessment: ExactAbsenceRecordAssessmentV1,
}

pub enum ExactAbsenceRecordAssessmentV1 {
    Legacy(UnusedCandidateDecisionV1),
    UnreadableCustody(CustodyReadRefusalV1),
    Custody {
        state: CustodyStateSnapshotV1,
        assessment: CustodyExactAbsenceAssessmentV1,
    },
}

pub enum CustodyExactAbsenceAssessmentV1 {
    IneligiblePopulation(IneligiblePopulationV1),
    CannotConstructSubject(CannotConstructSubjectV1),
    Assessed(UnusedCandidateDecisionV1),
}
```

**Fields stay private with read-only accessors.** Tests read through the
accessors, so the internal shape can change later without a test rewrite.

`IneligiblePopulationV1`, `CannotConstructSubjectV1` and `CustodyStateSnapshotV1`
are yours to define minimally here — they need only be closed enums/structs rich
enough for increment 2 to populate without changing this increment's public shape.
`CustodyStateSnapshotV1` must record the record's state (and the
`PreservationReasonV1` where the state carries one) without holding the whole
record.

**Do not add** an `InvalidStateClaimPair` arm. The canonical decoder already
rejects invalid required/forbidden claim pairs, so those records stay
`UnreadableCustody(Decode(..))`; a dormant arm for them would be unreachable by
construction.

### 2. The projection

`ExactAbsenceRecordAssessmentV1::decision() -> UnusedCandidateDecisionV1`,
**exhaustive, no wildcard**: `Legacy` and `Custody { assessment: Assessed(d) }`
return their contained decision; `UnreadableCustody`, `IneligiblePopulation` and
`CannotConstructSubject` project to `Refused`.

### 3. The checked scanner

Add a checked scan that returns the records **plus** enumeration completeness, the
canonical root, the pinned custody-root observation, and **the exact
descriptor-enumerated child name for each entry** (increment 2's sibling guard
needs the enumerated name, not a re-derived one — land it now so that increment
does not have to change this signature).

`scan_worktree_records(root) -> Vec<_>` **remains** as a compatibility wrapper that
deliberately erases the status for its existing legacy consumers. Add a comment
saying the erasure is intentional and that authorization-sensitive code must use
the checked report.

### 4. The production signature

```rust
pub fn sweep_orphans_with_exact_absence(
    root: &str,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactAbsenceSweepReportV1;
```

Production builds the report, logs the **projected** decisions exactly as it logs
them today, and discards it. `sweep_orphans` discards the report and then performs
its existing, independent legacy-removal / V3-classification scan **unchanged** —
report entries must never become inputs to that destructive second pass.

This is a public return-type change. Ordinary callers that discard the value stay
source-compatible; note the change in the handoff.

## Behavior preservation is the contract

For every input, the **projected decision for every record must be identical to
what this code produces today**. If you find yourself changing which records are
admitted, which are refused, or in what order guards run, you have left this
increment's scope — stop and say so.

The one intended observable difference is the return value's existence and the
truthful scan status. Log lines keep their current shape and values.

## Red-first battery — read this before writing tests

**This increment has NO genuine behavioral-red test, and that is correct.** Every
decision it produces is identical to the base's. Do **not** manufacture one, do
**not** contort the API so some test fails on `9aedf175`, and do **not** present a
compile failure as red evidence. State this plainly in the handoff.

Its exit evidence is instead:

- **Characterization.** Every existing fixture that exercises
  `sweep_orphans_with_exact_absence` asserts the projected decision per record is
  unchanged. These pass before and after; their job is to catch an accidental
  behavior change, and a reviewer should read them as such.
- **Truthful scan status**, which *is* new observable surface and must be tested
  directly:
  - a root that cannot be canonicalized ⇒ `Refused(CannotCanonicalize)`;
  - a root that cannot be enumerated ⇒ `Refused(CannotEnumerate)`;
  - a partially unreadable enumeration ⇒ `Incomplete { skipped_entries }` with the
    count correct;
  - a clean enumeration ⇒ `Complete`;
  - the three `CustodyRootObservationV1` values, each from a real construction if
    it can be built, and named as not-executed if it cannot.
- **Projection totality.** `decision()` is exhaustive over every arm, asserted
  arm-by-arm in a table-driven test so a later arm cannot be added without a
  failing test.
- **Effect freedom**, per the audit below.

### Effect-freedom evidence

Byte snapshots are **not** sufficient on their own — they prove final-state
equality and cannot exclude a helper that mutates and restores. The principal
evidence is a **bounded transitive source audit** from `sweep_orphans_with_exact_absence`,
the checked scanner, and both `ExactAbsenceProbeV1` methods including
`HostGitWorktree::observe_exact_absence`.

Allowed leaves: bounded reads and decoding, descriptor and metadata observation,
canonicalization and identity checks, `git rev-parse`, `git worktree list
--porcelain -z`, allocation, collection, tracing.

The audit must show **no edge** to provider removal or pruning, `remove_dir_all`,
unlink or rename, custody publication or replacement, settlement, transitions, or
backend cleanup. Record the audit in the handoff as a call-path list. Byte
snapshots stay as corroborating regressions.

## Who runs which gate

Your container has **no compile loop**, so it cannot produce final gate totals. Do
not fabricate them and do not treat that as licence to skip them.

- **You**: write the code and tests, state per test whether you executed it, and
  record whatever your verify stage reports.
- **The operator, on the host**: `cargo fmt --all -- --check`; `cargo clippy
  --workspace --all-targets --locked -- -D warnings`; `CARGO_INCREMENTAL=0 cargo
  test --workspace --locked --no-fail-fast`.
- **The handoff carries a clearly-marked operator-evidence section** with a pending
  placeholder per item. Leave the placeholders; the operator fills them before
  closure. There is no base-red item for this increment.

## On evidence

`bridge-core` compiles for Windows in CI while `liveness` and
`namespace_transaction` are `#[cfg(unix)]`. This lane has lost five landing rounds
to that boundary. Anything that becomes unused on non-unix needs
`#[cfg_attr(not(unix), allow(dead_code))]`; the established shape is commit
`790b4191`. State what you gated. **Do not add a `bridge-core` surface in this
increment.**

Three tests in the adjacent slice looked like evidence and were not; one passed on
macOS/APFS and on this container's overlayfs and failed only on ubuntu/ext4,
because it depended on inode reuse after unlink-and-recreate. Prefer tests whose
outcome does not depend on filesystem allocation behaviour. Where a status test
must construct real filesystem state (an unreadable directory, a vanished root),
say which environments can prove it.

**Falsification license.** Every anchor, symbol name and behavioural claim above
was measured by the operator at `9aedf175` and may be wrong; the repository is the
authority. If `sweep_orphans_with_exact_absence` already returns a value, if
`scan_worktree_records` already reports completeness, or if a named type already
exists — say so plainly with the evidence and stop rather than forcing the change
to fit this description. Finding the work smaller than described is a good
outcome. Not open to revision: the T3a-decides / T3b-acts split, and the exclusion
of ownership.

## Acceptance Criteria

1. `sweep_orphans_with_exact_absence` returns `ExactAbsenceSweepReportV1`;
   `sweep_orphans` discards it and its existing independent second scan is
   unchanged.
2. The report vocabulary above exists with **private fields and read-only
   accessors**, and `decision()` is exhaustive over every arm with no wildcard.
3. The checked scanner reports enumeration completeness, the canonical root, the
   custody-root observation, and the exact descriptor-enumerated child name per
   entry. `scan_worktree_records` remains as a documented compatibility wrapper.
4. **No decision changes.** The projected decision for every record is identical to
   the base's for every input, and characterization tests assert it.
5. The unconstructed arms exist, are documented as increment-2 wiring, and are not
   removed, `cfg(test)`-gated, or fake-constructed. No `InvalidStateClaimPair` arm.
6. No ownership input, parameter, or variant anywhere.
7. Scan-status tests cover `Complete`, `Incomplete { skipped_entries }` with a
   correct count, both `ExactAbsenceRootRefusalV1` values, and the
   `CustodyRootObservationV1` values that can be constructed — each marked executed
   or not-executed honestly.
8. `decision()` totality is asserted arm-by-arm in a table-driven test.
9. The handoff records the bounded transitive effect-freedom audit as a call-path
   list, and states plainly that this increment has no genuine behavioral-red test
   and why.
10. No custody state, transition, publication, settlement, deletion, or CLI
    call-site behavior changes; no new `bridge-core` surface; no async proof trait;
    no change to `compare_path_identities` or `host_git.rs`'s proof.
11. The handoff is created at
    `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-increment1-handoff.md`, with
    the marked operator-evidence section and its pending placeholders.
12. `git diff --numstat 9aedf175..HEAD` at most **300** changed lines including
    tests and the handoff, measured on a **clean, fully committed worktree** — the
    command ignores staged, unstaged and untracked bytes, so an uncommitted handoff
    would let a breach read as green. A breach requires an explicit pre-closure
    operator waiver.
13. Report test totals as the count of test binaries plus doc-test suites, not by
    summing `test result:` lines — a bridge-core test re-executes the test binary as
    a filtered subprocess and its nested harness line inflates a naive sum.

## Files

- `crates/bridge-worktree/src/sweep.rs` — the vocabulary, the checked scanner, the projection, the tests.
- `crates/bridge-worktree/src/custody.rs` — read for the state and reason enums; **do not modify**.
- `crates/bridge-worktree/src/host_git.rs` — the probe implementation; read only, for the effect-freedom audit.
- `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-increment1-handoff.md` — the handoff to create.

## Spec Refs

Not in your checkout — reproduced above where load-bearing, and their absence is
not a missing input:

- `docs/superpowers/plans/2026-08-18-r2f1b-3d-t3a-rescope-design.md` — the design
  this increment implements, including increments 2 and 3 which this one exists to
  make possible.

## Commit Message

feat(worktree): return a typed exact-absence sweep report

sweep_orphans_with_exact_absence returned () and only logged each decision, so
nothing could assert an exact typed assessment through the real production
traversal. It now builds and returns a total report — scan status, per-record
assessment, and an exhaustive projection back to the existing decision — while
production continues to log the projected decisions and discard the value.

The scanner now also reports the truth about its own enumeration: whether the root
could be canonicalized and enumerated, how many entries were skipped, and what was
observed of the custody root's identity. scan_worktree_records remains as a
compatibility wrapper that deliberately erases that status for its legacy
consumers.

Deliberately behavior-preserving: every projected decision is identical to before,
and characterization tests pin that. The vocabulary includes arms this change never
constructs, because the increment that starts constructing them can only produce
genuine behavioral-red evidence if the types already compile on its base — a test
naming a type that does not yet exist fails to compile rather than to fail, and a
compile error is not behavioral evidence.

No ownership input or variants: the production boot sweeps run before any
WorktreeBackend exists, so this layer cannot truthfully observe in-memory ownership
and does not pretend to.
