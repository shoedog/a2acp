---
task-type: implement
---

# Author the increment 2 task spec — population admission and construction guards

## Description

Write a complete, dispatchable **task spec** for T3a increment 2. Not a design
note — a spec an implementer can execute.

T3a increment 1 and slice B are merged. `sweep_orphans_with_exact_absence`
returns a populated report with real root observations. But **every readable
custody record still reaches the probe and becomes an assessment**, whatever its
state. Increment 2 installs the admission rule that stops that.

### The single production hole

`crates/bridge-worktree/src/sweep.rs`, in `report_exact_scan_projection_row`:

```rust
ScannedWorktreeRecordV1::Custody(record) => {
    ExactAbsenceRecordAssessmentV1::Custody(CustodyRecordAssessmentV1::new(
        CustodyStateSnapshotV1::from(&record.state),
        CustodyExactAbsenceAssessmentV1::Assessed(decision),
    ))
}
```

Every custody record becomes `Assessed(decision)`. Increment 2 must interpose
admission before the **probe** — which is **not** here.
`report_exact_scan_projection_row` runs inside `into_report`, strictly after
`project_exact_scan_result` has already called the probe for every row, so
admission installed at this construction site is a post-hoc filter that cannot
suppress a single probe call. The sole custody path to the probe is
`decide_unused_custody_record` → `decide_unused_candidate`; admission belongs there.
`[CORRECTED 2026-08-21 — the original pointed at this construction site. Both
authors traced the call path and refuted it; verified on main. The error would have
produced a cosmetic gate passing any decision-only test.]`

### The 16 populations

Ten states in `WorktreeCustodyStateV1`, with `PreservationUnknown` carrying six
`PreservationReasonV1` values, and `ProtectionPrepared` splitting on whether a
claim is present — sixteen populations. Verify that arithmetic against the code
before relying on it.

The admission rule, from the rescope design:

- `ProtectionPrepared` **without** a claim ⇒ `IneligiblePopulationV1::BareProtectionPrepared`.
  Its claim is schema-**optional**, so this is not a malformed or missing-required
  claim; it is a legitimate record that is simply not a candidate.
- `ProtectionPrepared` **with** a claim ⇒ proceed to construction.
- `PreservationUnknown { MaterializationInFlight }` ⇒ proceed to construction.
- The other five `PreservationUnknown` reasons, `PreservationPrepared`,
  `Preserved`, and every claim-**forbidden** state ⇒
  `IneligiblePopulationV1::StateNotCandidate`.

That is **2 candidate populations and 14 ineligible ones**. Today all sixteen are
assessed, so this is a large narrowing — and it narrows toward refusal, which is
the safe direction.

**Do not add an `InvalidStateClaimPair` arm.** The canonical decoder already
rejects invalid required/forbidden claim pairs, so those records remain
`UnreadableCustody(Decode(..))`. A dormant arm would be unreachable by
construction.

### The construction guards

Populate the guard arms of `CannotConstructSubjectV1` that production does not
yet build: `RecordedWorktreePathNotAbsolute`, `OutsideSweepRoot`, and
`RecordFileNotExpectedSibling`. `ClaimAuthorityUnavailable` is **also dormant**: its only two
`ClaimAuthorityUnavailableV1::new` call sites are tests, so all four arms are
unbuilt in production. It stays dormant here — retyping the string-valued
claim-construction errors is increment 3's named work.
`[CORRECTED 2026-08-21 — the original said it already had production construction.
Both authors refuted it; verified on main.]`

Typed guard **precedence** matters and must be stated once, unambiguously: when a
record could fail more than one guard, the spec must fix which one it reports, so
two implementations cannot disagree. Exact child-name matching belongs here too.

### Behavioral red — this increment has genuine runtime red

Unlike A2a, increment 2 changes decisions. The rescope design names the evidence:

- canonical `Preserved` + complete claim + `BothAbsent` moves from
  `Assessed(Authorized)` to `StateNotCandidate`, **with zero probe calls**;
- bare `ProtectionPrepared` becomes `BareProtectionPrepared`;
- the expected-sibling symlink alias is distinctly refused;
- claim-bearing `ProtectionPrepared` and materialization-in-flight-unknown still
  reach the recording probe **once**;
- invalid persisted claim pairs stay unreadable rather than becoming constructed
  assessments.

The zero-probe-call assertions are the load-bearing ones: they prove admission
runs *before* the probe, not that its result is filtered afterwards. A test that
only checks the final decision cannot tell those apart.

Require a frozen genuine-red control — a test-only patch against a recorded base
tree, its SHA-256, and the command that runs it — as slice B and A2b both did.

### SCOPE RULING — readiness does NOT flip in this increment

Earlier specs said the readiness gate and the admission rule both belong to
increment 2. **The owner has ruled otherwise: `EXACT_ABSENCE_POLICY_READY_V1`
stays `false`.**

The evidence, measured on `main`:

- `effective()` has **zero production consumers**. Its only call sites are three
  test assertions. Flipping readiness would therefore change no behavior at all
  today.
- `entry_is_effectively_authorized_for_policy` is
  `policy_ready && has_authoritative_scan() && …`. Since slice B,
  `has_authoritative_scan()` returns `true` for a healthy root, so readiness is
  the **sole remaining gate**. Flipping it removes the last one.

So flipping buys nothing now and costs the final guard. It belongs with the slice
that actually consumes `effective()` — T3b — under that slice's own review.

State this in the spec as a scope fence, with the reason, so a later reader does
not "fix" it back.

### Scope fences

Increment 2 does **not**:

- set `EXACT_ABSENCE_POLICY_READY_V1` to `true`, or change `effective()` or
  `entry_is_effectively_authorized_for_policy`;
- change the public signature of `sweep_orphans_with_exact_absence` or the report
  vocabulary in `report.rs` — the A1 types are settled and increment 2 fills arms
  that already exist;
- change `classify_root_observations`, the retained-descriptor enumerator, or any
  root-observation behavior landed by slice B;
- add ownership, locking, transition, unlink, or removal authority. **T3a decides
  and reports; T3b acts.** A later actor must re-open, re-read, re-bind, and
  re-prove exact absence under its own lock;
- repair the Unix-only separator guard in `is_custody_record_name`, characterized
  by A2a-2 and deliberately unrepaired.

### Behavior that must not change

The A2a-2 characterization scenarios and slice B's root-observation tests must
still hold. Where an existing test asserts a decision for a record whose state is
now ineligible, that assertion legitimately changes — **name every such test in
the handoff and justify each one individually.** An unexplained colour change
elsewhere is a behavior change: stop and report.

### Sizing

The rescope design set a hard cap of **450 changed lines including tests**. That
figure predates every measurement this lane has since taken, so treat it as an
anchor to test, not a budget to trust.

Measured in this crate: roughly 28–35 nonblank lines per test, higher for
filesystem fixtures. Sixteen populations with zero-probe assertions is a lot of
test surface. Size it honestly; if the honest estimate materially exceeds 450,
say so and propose a split rather than compressing evidence to fit. Two slices in
this lane have already been split for exactly that reason, and both splits were
right.

### Operator-owned gates

The implement container's egress cannot fetch the pinned `a2a-lf` dependency, so
`cargo` cannot build there. The implementer makes the implementation-candidate
commit and authors a handoff carrying six unticked `PENDING OPERATOR` gate lines;
the host operator runs the gates and makes the handoff-only evidence commit.
Reporting a gate as blocked is correct; inventing a total is not.

### Environment facts

Your working tree is at `main`, and the repository is authoritative over every
claim above — several claims in earlier briefs for this lane turned out false and
were caught by authors reading the code. Verify each anchor: the assessment
construction site, the state and reason enums, `claim_presence`, which
`CannotConstructSubjectV1` arms production already builds, and the `effective()`
call sites.

You cannot read anything outside the repository, and the spec must never name a
path outside it — the implementer runs in a container with only the code tree
mounted.

## Acceptance Criteria

This authoring task is complete when the emitted spec: states the admission table
for all sixteen populations unambiguously; fixes guard precedence; requires the
zero-probe-call evidence and a frozen red control; carries the readiness scope
fence with its reason; names the tests whose assertions legitimately change; and
gives a per-row sizing worksheet with a falsification license.

## Output contract

Emit the complete increment 2 task spec, with front matter `task-type: implement`
and sections `## Description`, `## Acceptance Criteria`, `## Files`,
`## Spec Refs`, `## Commit Message`.

Prefer precision over length. A spec an implementer can execute without guessing
beats a longer one that restates context.
