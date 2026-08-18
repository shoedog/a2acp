---
task-type: implement
---

# R2f1b 3d T3a slice 1 — population boundary and advisory vocabulary

## Description

First of two slices rebuilding T3a on a tree where most of T3a's original charter
already landed. Base: `main` = `9aedf175`.

**T3a DECIDES; T3b ACTS.** This slice performs **no record mutation whatsoever**
and adds no transition-table edge. Its entire output is a typed decision value.
Designing or building T3b's lock window, the `UnusedSettled` publication,
descriptor-safe removal, or marker-removal authority is out of scope.

### What already exists — do not rebuild it

The path-identity slice landed most of T3a's substrate in
`crates/bridge-worktree/src/sweep.rs`:

- `ExactAbsenceCandidateV1` with `from_legacy` and `from_claim`, binding both the
  source and the Git common-directory identity.
- `ExactAbsenceObservationV1 { TargetPresent, RegisteredButAbsent, BothAbsent }`.
- `ExactAbsenceProbeV1`, a **synchronous** trait over the host capability. The
  seam question is settled — do not add an async trait, a nested runtime, or
  another public host-Git abstraction.
- `decide_unused_candidate`, plus the two population adapters
  `decide_unused_legacy_sidecar` and `decide_unused_custody_record`.
- `sweep_orphans_with_exact_absence`, reached from `sweep_orphans`.

Underneath, `bridge_core::fs_custody::compare_path_identities` is the landed
tri-state path-identity primitive. **Do not change it, and do not change
`host_git.rs`'s proof.**

### The defect this slice closes

`decide_unused_custody_record` constructs its candidate from `record.claim` **with
no check on `record.state`**. Every claim-bearing V3 record therefore reaches the
exact-absence proof. A `Preserved` record whose target vanished externally can
consequently produce a positive result — a fail-open in a fail-closed contract.
It is currently harmless only because the decision is effect-free; it becomes
destructive the moment T3b acts on it.

Admission must be restricted to the chartered population **before** candidate
construction and **before** any probing.

### Owner decisions already taken — implement these, do not reopen

1. **Bare `ProtectionPrepared` residue refuses.** A process that crashes after
   publishing `ProtectionPrepared` may leave a record with no claim, and the
   companion preparation journal carries only a schema version, flight id and
   state — no candidate authority, and so insufficient for a positive proof. T3a **refuses** that residue. Durable candidate
   evidence (a preparation-journal v2, or a record digest) is a **separately
   chartered prerequisite for T3b** and is explicitly NOT built here.
2. **Do not infer pre-target status from a state name.** Assess every
   `PreservationUnknown { reason: MaterializationInFlight }` record, and label the
   result **advisory**. Do not require, or attempt to reconstruct, proof that the
   original add outcome was provably absent.
3. **Do not manufacture a red result** for the named exit gate. See the battery
   below for exactly which tests must be behaviorally red and which are accepted
   as regression coverage.

### The result is advisory — the naming carries that

Boot cannot honestly claim global non-ownership: its backend does not exist yet,
and another process may own the same target. The strongest answer this slice may
produce is therefore **`ReadyForLockedReproof`**, never `Authorized`. T3b must
re-run the complete proof inside its own action window; nothing produced here is
reusable mutation authority. Name the variants accordingly and say so in a doc
comment on the type.

## What to build

In `crates/bridge-worktree/src/sweep.rs`, split **exact-absence observation** from
**unused-candidate assessment**, and give refusals explicit reasons:

```rust
pub enum ExactAbsenceRefusalV1 {
    TargetPresent,
    RegisteredButAbsent,
    CannotProve,
}

pub enum ExactAbsenceDecisionV1 {
    ProvedAtObservation,
    Refused(ExactAbsenceRefusalV1),
}

pub struct UnusedCandidateSubjectV1 {
    pub custody_id: WorktreeCustodyIdV1,
    pub checkout_fingerprint: Sha256HexV1,
    pub candidate: ExactAbsenceCandidateV1,
}

pub enum UnusedCandidateRefusalV1 {
    IneligiblePopulation,
    CannotConstructSubject,
    LocallyOwned,
    OwnershipCannotProve,
    ExactAbsence(ExactAbsenceRefusalV1),
}

pub enum UnusedCandidateAssessmentV1 {
    ReadyForLockedReproof(UnusedCandidateSubjectV1),
    Refused(UnusedCandidateRefusalV1),
}
```

**One proof definition, unchanged in substance:**

```rust
fn decide_exact_absence(
    candidate: &ExactAbsenceCandidateV1,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactAbsenceDecisionV1;
```

with the table it already implements: `BothAbsent` ⇒ `ProvedAtObservation`;
`TargetPresent` and `RegisteredButAbsent` ⇒ the corresponding refusal; probe
`Err` or ambiguity ⇒ `CannotProve`. **A probe `Err` is never absence.**

**No public population enum.** Only one population is eligible in this slice, so do
**not** publish a `UnusedCandidatePopulationV1` advertising `ProtectionPrepared` as
a possible subject when the admission table makes it refusal-only. Prefer a
**private** projection returning `Result<UnusedCandidateSubjectV1,
UnusedCandidateRefusalV1>`. Add a population enum later, when a second population
actually becomes eligible.

`LocallyOwned` and `OwnershipCannotProve` are part of the vocabulary this slice
defines but are **not constructed here** — slice 2 wires the backend ownership
observer that produces them. State that in a comment so a reviewer does not read
them as dead vocabulary.

**Signature migration — state it exactly, do not improvise.** `decide_unused_candidate`
today is `pub fn decide_unused_candidate(candidate: &ExactAbsenceCandidateV1,
recovery_owned: bool, probe: &dyn ExactAbsenceProbeV1) -> UnusedCandidateDecisionV1`,
and it refuses when `recovery_owned` is true. **Measured: both production call sites
pass a hardcoded `false` and no caller anywhere passes `true`**, so no current path
loses a refusal — but the capability must not silently disappear either. Therefore:

- Keep a refusal path for recovery ownership reachable through the V3 assessment,
  even though nothing constructs it in this slice, so slice 2 wires an observer into
  an existing hole rather than reintroducing a dropped concept.
- If you remove or rename `decide_unused_candidate`, say so explicitly in the
  handoff and show that every caller moved. Do not leave two proof entry points.
- **V3 and legacy dispatch separately.** The legacy adapter keeps returning its own
  decision type; do not force legacy and V3 through one combined match. The
  top-level sweep loop may map both to a common reporting shape, but the two
  branches stay distinct.

**Population projection — the admission table is exhaustive, and it is a table.**
Project a scanned V3 record to an admission outcome **before** constructing an
`ExactAbsenceCandidateV1` and **before** calling the probe. Admission is decided by
`(state, claim presence)` jointly, not by state alone:

| Record state | Claim | Outcome |
|---|---|---|
| `PreservationUnknown { reason: MaterializationInFlight }` | present (schema `Required`) | **eligible** — construct subject, then probe |
| `ProtectionPrepared {}` | **absent** | refuse `CannotConstructSubject` |
| `ProtectionPrepared {}` | **present** | refuse `IneligiblePopulation` |
| `PreservationUnknown` with any other `reason` | present | refuse `IneligiblePopulation` |
| `PreservationPrepared`, `Preserved` | present | refuse `IneligiblePopulation` |
| `UnusedSettled`, `Materializing`, `LiveProtected`, `DeleteAuthorized`, `Removed`, `RecoveredLive` | forbidden | refuse `IneligiblePopulation` |

**`ProtectionPrepared` is the schema's one `ClaimPresenceV1::Optional` state**
(`custody.rs`, `claim_presence`), so a claim-bearing `ProtectionPrepared` record is
schema-valid and would otherwise be constructible and probeable. Both forms refuse.
The refusal is not conditional on the claim being unusable — do **not** write a rule
of the form "refuse because no claim exists", because that rule silently admits the
claimed form.

Handle the enumeration **exhaustively over the state enum**, so that adding a state
later fails to compile rather than defaulting into eligibility. No `_ =>` catch-all
that maps to eligible.

**Guards that must survive the refactor.** `decide_unused_custody_record` already
refuses when the worktree is not under the sweep root (`worktree_under_root`) and
when the scanned record file is not the canonical custody-record sibling of that
worktree (the `canonicalize(record_file) == canonicalize(custody_record_path(..))`
check). **Both must remain, and both must run ahead of subject construction and
probing.** Map each to a fixed refusal reason and cover each with a zero-probe
regression test. A refactor that drops either would return a positive result for an
out-of-root or mismatched record while still satisfying every other test here.

**Legacy stays separate.** Do not fold `decide_unused_legacy_sidecar` into the V3
population projection; keep its behavior as-is.

**Effect freedom is byte-for-byte.** No path in this slice may write, rename, or
unlink a custody record, a marker, a sidecar, or a checkout directory.

## Red-first battery

Per owner decision 3, be exact about what is genuinely red and what is not.

**Genuinely red on `9aedf175` — these must fail on the unmodified tree:**

- `only_materialization_inflight_records_enter_unused_marker_proof` — construct
  **real** claim-bearing records in `Preserved`, `PreservationPrepared`, and a
  `PreservationUnknown` with a reason other than `MaterializationInFlight`, plus
  the eligible `PreservationUnknown { reason: MaterializationInFlight }`. Assert
  with a counting probe that only the eligible record causes a probe invocation.
  Red today because every claim-bearing record routes to the proof.
- `claim_bearing_protection_prepared_refuses_before_probing` — a **claim-bearing**
  `ProtectionPrepared` record. `ProtectionPrepared` is the schema's one
  `ClaimPresenceV1::Optional` state, so this record is valid, is constructible
  today, and **currently reaches the probe**. It must newly refuse
  `IneligiblePopulation` with **zero** probe invocations. This is the second
  genuinely-red case.

**Guard regressions — zero probe invocations, each behaviorally red only if the
corresponding guard is dropped, so treat them as regression coverage:**

- An eligible-state record whose worktree is not under the sweep root.
- An eligible-state record whose scanned file is not the canonical custody-record
  sibling of its worktree.

**Regression and exit coverage, not claimed as red:**

- `unused_candidate_settles_only_after_exact_absence` — the named exit gate. Four
  arms: target-present refuses; registered-but-absent refuses; probe-error
  refuses; both-absent yields `ReadyForLockedReproof`. **"Settles" means the proof
  result only.** Every arm must run through the **real V3 production adapter and
  the scanned-record sweep path** with a programmable probe — not by calling
  `decide_exact_absence` directly, which would pass even if the adapter mutated
  records. Snapshot record bytes and the enclosing directory entries before and
  after each arm and assert both unchanged. Say plainly in the handoff that this
  is regression coverage and that the two tests above are the red evidence.

**Degraded-claim handling — specify it, do not test the impossible.** The original
draft of this spec mandated a `degraded_materialization_marker_refuses_without_probing`
test. That test has **no constructible red fixture** and has been withdrawn:
`PreservationUnknown` requires a claim (`ClaimPresenceV1::Required`), so an absent
claim is schema-invalid; an unbound common-directory claim already refuses without
probing today; and a target-degraded but authority-bound claim remains
constructible and is legitimately probed. Instead, state per field which degraded
claims are constructible and which already refuse, and cover the constructible ones
as regression coverage only.

### Proving projection happens BEFORE construction

Counting probe invocations does **not** prove ordering. A candidate-first
implementation can construct `ExactAbsenceCandidateV1::from_claim` — which performs
filesystem and Git authority binding — then reject the state, and still record zero
probe calls. AC1 requires refusal before construction, so it needs its own evidence.
Use **one** of:

- an instrumentable construction seam that counts `from_claim` invocations, asserted
  to be zero for every ineligible row; **or**
- an ineligible record whose source/common-dir authority is invalid, so that a
  candidate-first implementation fails distinctly rather than returning
  `IneligiblePopulation`. Assert the exact refusal reason is `IneligiblePopulation`
  and not a binding failure.

State which you chose and why it discriminates.

### What the snapshots do and do not prove

Before/after byte and directory-entry snapshots prove **final-state equality**. They
cannot detect a transient write, rename, or unlink that is restored before the
snapshot. Describe them in the handoff as final-state evidence. For the stronger
claim that *no mutation operation occurred*, either add an operation-recording seam
or give source-level verification that the touched paths contain no write, rename,
or unlink call — and say which you did.

## Who runs which gate

Your container has **no compile loop**, so it cannot produce base-red output or
final gate totals. Do not treat that as a reason to skip them, and do not fabricate
them.

- **You** write the tests, state per test whether you executed it, and record
  whatever your verify stage reports.
- **The operator runs, on the host:** the exact pre-change failure output for both
  genuinely-red tests against unmodified `9aedf175`, and the full post-change gate
  (`cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --locked
  -- -D warnings`; `cargo test --workspace --locked --no-fail-fast`).
- **The handoff must contain a clearly-marked section** for that operator-supplied
  evidence, with a placeholder line per item saying it is pending operator
  execution. Leave the placeholders in rather than guessing values; the operator
  fills them before closure.

## On evidence

Your container has **no compile loop** and its `verify: PASS` has repeatedly
failed to hold on the host for this subsystem. State **per test** whether you
executed it; do not present a green verify as evidence a test passes.

This lane has shipped three tests that looked like evidence and were not. One
passed on macOS/APFS *and* on this container's overlayfs and failed only on
ubuntu/ext4, because it depended on inode reuse after unlink-and-recreate. So:
prefer tests whose outcome does not depend on filesystem allocation behaviour;
where a test must construct real filesystem state, say which environments can
prove it. Pure decision-table and projection tests should be platform-neutral.

`bridge-core` compiles for Windows in CI while `liveness` and
`namespace_transaction` are `#[cfg(unix)]`. Do not reference those from code
reachable on non-unix without a `#[cfg(unix)]` guard on the referencing item, and
gate anything that becomes unused on non-unix with
`#[cfg_attr(not(unix), allow(dead_code))]`. The established shape is commit
`790b4191`. State what you gated. **Do not add a new `bridge-core` surface.**

**Falsification license.** Every anchor, symbol name and behavioural claim above
was measured by the operator at `9aedf175` and is a claim you may disprove. The
repository is the authority. In particular: if `decide_unused_custody_record` does
in fact gate on `record.state`, if the eligible-state projection already exists,
or if a named symbol does not exist under that name — **say so plainly with the
evidence and stop rather than forcing the change to fit this description.**
Finding the work smaller than described is a good outcome. The one thing not open
to revision is the T3a-decides / T3b-acts split.

## Acceptance Criteria

1. The admission table above is implemented **exhaustively over the state enum**,
   keyed on `(state, claim presence)` jointly. **Both** forms of
   `ProtectionPrepared` refuse; neither is constructible or probeable. No `_ =>`
   catch-all maps to eligible.
2. Refusal happens **before** `ExactAbsenceCandidateV1` construction and **before**
   any probe call, and the handoff shows the evidence chosen from "Proving
   projection happens BEFORE construction" — a counting probe alone does not
   satisfy this.
3. `worktree_under_root` and the canonical record-file/sibling check both survive,
   both run ahead of subject construction, each maps to a fixed refusal reason, and
   each has a zero-probe regression test.
4. Exact-absence observation and unused-candidate assessment are separate types,
   with exactly **one** exact-absence proof definition. V3 and legacy dispatch
   remain distinct branches.
5. The positive result is named `ReadyForLockedReproof` (or an equally explicit
   non-authority name), and its doc comment states it is advisory and that T3b must
   re-prove under its lock.
6. A recovery-ownership refusal path remains reachable in the V3 assessment for
   slice 2 to wire; `LocallyOwned` and `OwnershipCannotProve` are documented as
   slice-2 wiring and constructed nowhere here. If `decide_unused_candidate` is
   removed or renamed, the handoff shows every caller moved and that only one proof
   entry point remains.
7. **Zero mutation.** No custody record, marker, sidecar, or checkout directory is
   written, renamed, or unlinked on any path this slice touches. The named exit gate
   runs all four arms through the **real V3 adapter and scanned-record sweep path**
   and asserts unchanged record bytes and directory entries. The handoff states
   plainly that snapshots are final-state evidence and gives the stronger
   no-operation evidence it chose.
8. No new edge in the frozen custody transition table; no new
   `ScannedWorktreeRecordV1` variant; no public population enum; no async
   exact-absence trait; no nested runtime; no change to `compare_path_identities` or
   to `host_git.rs`'s proof; no preparation-journal or durable-schema change.
9. Legacy sidecar behavior is unchanged.
10. Both genuinely-red tests exist —
    `only_materialization_inflight_records_enter_unused_marker_proof` and
    `claim_bearing_protection_prepared_refuses_before_probing` — and the named exit
    gate exists and is honestly labelled regression coverage. The withdrawn
    degraded-claim test is not reintroduced; degraded claims are specified
    field-by-field instead.
11. The handoff is written to
    `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-slice1-handoff.md` (create it),
    and contains the marked operator-evidence section with pending placeholders per
    "Who runs which gate".
12. `git diff --numstat 9aedf175..HEAD` at most **400** changed lines including tests
    and the handoff. The cap was raised from 250 after the spec review added the
    exhaustive admission table, two surviving guards with regression tests, the
    production-path exit gate, and the construction-ordering evidence. A breach still
    requires an explicit pre-closure operator waiver.
13. Report test totals as the count of test binaries plus doc-test suites, not by
    summing `test result:` lines — a bridge-core test re-executes the test binary as
    a filtered subprocess and its nested harness line inflates a naive sum.

## Files

- `crates/bridge-worktree/src/sweep.rs` — the vocabulary, the population projection, the split, and the tests.
- `crates/bridge-worktree/src/custody.rs` — read for the state and reason enums; **do not modify**.
- `crates/bridge-worktree/src/host_git.rs` — the probe implementation; read only.
- `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-slice1-handoff.md` — the handoff to create.

## Spec Refs

- `docs/superpowers/plans/2026-08-18-r2f1b-3d-t3a-rebuild-design.md` — the design
  this slice implements, including the ownership seam that slice 2 will build.
- `docs/superpowers/plans/2026-08-17-r2f1b-3d-t3a-task.md` — T3a's original
  charter. **Historical**: written before the primitive landed, so its "what to
  build" substantially describes work already on `main`.

## Commit Message

feat(worktree): gate the unused-candidate proof to its chartered population

decide_unused_custody_record constructed its candidate from record.claim with no
check on record.state, so every claim-bearing V3 record reached the exact-absence
proof. A Preserved record whose target vanished externally could therefore produce
a positive result — a fail-open in a fail-closed contract, harmless only while the
decision stays effect-free and destructive the moment T3b acts on it.

Admission is now projected to a population before any candidate is constructed and
before the probe is called. Only PreservationUnknown with reason
MaterializationInFlight is eligible. Bare ProtectionPrepared residue refuses: a
crash after that publication leaves no claim, and the preparation journal carries
only a flight id and state, which is not authority for a positive proof.

Exact-absence observation and unused-candidate assessment are now separate types
with explicit refusal reasons, over a single unchanged proof definition. The
positive result is named ReadyForLockedReproof rather than Authorized, because
boot cannot honestly claim global non-ownership — its backend does not exist yet
and another process may own the same target. Nothing here is reusable mutation
authority; T3b re-proves inside its own lock window.

No record is written, renamed or unlinked on any path. No transition-table edge,
no new scan variant, no async proof trait, and no change to the path-identity
primitive.
