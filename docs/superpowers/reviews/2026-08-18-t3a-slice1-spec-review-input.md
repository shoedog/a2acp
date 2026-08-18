---
task-type: spec-review
---

# Spec review — R2f1b 3d T3a slice 1

## Description

Review the implementation task spec reproduced verbatim below, **before** it is
dispatched to an implement lane. Approve it or send it back.

The repository at the session cwd is checked out at `main` = `9aedf175`, which is
the base the spec targets. **The spec itself is not in this checkout** — it lives
on a planning branch, which is why it is inlined here. Its absence from the tree
is not a missing input.

### Context you need

The worktree lane has a slice, 3d T3a, split into a deciding half and an acting
half. **T3a decides and performs no record mutation; T3b acts.** This spec is the
first of two slices rebuilding T3a's residual, after a shared tri-state
path-identity primitive landed and absorbed most of T3a's original charter.

The defect the slice closes: `decide_unused_custody_record` in
`crates/bridge-worktree/src/sweep.rs` constructs its exact-absence candidate from
`record.claim` with no check on `record.state`, so every claim-bearing V3 record
reaches the proof. A `Preserved` record whose target vanished externally can
therefore produce a positive result. Fail-open in a fail-closed contract, harmless
only while the decision is effect-free.

Three scope questions were already decided by the operator and are **not open**:
bare `ProtectionPrepared` residue refuses rather than gaining a durable-evidence
schema in this slice; the eligible population is assessed without inferring
pre-target status from a state name; and the named exit gate is accepted as
regression coverage rather than having the API contorted to force it red.

**This lane's failure mode is contracts, not coding.** Three prior rules in the
adjacent slice were invented, rejected, and one refuted, because a spec demanded a
proof it also made impossible. Three tests in the adjacent slice looked like
evidence and were not — one passed on macOS/APFS and on the implement container's
overlayfs and failed only on ubuntu/ext4. Read this spec with those failure modes
in mind: your highest-value finding is an instruction that cannot be satisfied as
written, or a test that would pass without proving what it claims.

---

## The spec under review

```markdown
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
   publishing `ProtectionPrepared` leaves a record with no claim, and the
   companion preparation journal carries only `{flight_id, state}` — insufficient
   authority for a positive proof. T3a **refuses** that residue. Durable candidate
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

pub enum UnusedCandidatePopulationV1 {
    ProtectionPrepared,
    MaterializationInFlightMarker,
}

pub struct UnusedCandidateSubjectV1 {
    pub population: UnusedCandidatePopulationV1,
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

`LocallyOwned` and `OwnershipCannotProve` are part of the vocabulary this slice
defines but are **not constructed here** — slice 2 wires the backend ownership
observer that produces them. State that in a comment so a reviewer does not read
them as dead vocabulary.

**Population projection.** Project a scanned record to a population before
constructing a subject:

- `PreservationUnknown { reason: MaterializationInFlight }` ⇒
  `MaterializationInFlightMarker`, eligible.
- `ProtectionPrepared {}` ⇒ the `ProtectionPrepared` population, but with no claim
  it cannot yield a constructible subject ⇒ `CannotConstructSubject` (decision 1).
- Every other state ⇒ `IneligiblePopulation`. **Refuse before constructing a
  candidate and before calling the probe.**

**Legacy stays separate.** Do not fold `decide_unused_legacy_sidecar` into the V3
population projection; keep its behavior as-is.

**Effect freedom is byte-for-byte.** No path in this slice may write, rename, or
unlink a custody record, a marker, a sidecar, or a checkout directory.

## Red-first battery

Per owner decision 3, be precise about what is genuinely red:

**Behaviorally red on `9aedf175` — these must fail on the unmodified tree:**

- `only_materialization_inflight_records_enter_unused_marker_proof` — construct
  **real** claim-bearing records in `Preserved`, `PreservationPrepared`, an
  unrelated `PreservationUnknown` reason, and the eligible
  `PreservationUnknown { reason: MaterializationInFlight }`. Assert that **only**
  the eligible record causes the probe to be invoked, using a counting probe. This
  is red today because every claim-bearing record routes to the proof.
- `degraded_materialization_marker_refuses_without_probing` — an eligible-state
  record whose claim is absent or degraded, written through the real
  provider-error writer shape. Assert `CannotConstructSubject`, **zero** probe
  invocations, and unchanged record bytes.

**Accepted as regression and exit coverage, not claimed as red:**

- `unused_candidate_settles_only_after_exact_absence` — the slice's named exit
  gate. A real eligible record, with four arms: target-present refuses;
  registered-but-absent refuses; probe-error refuses; both-absent yields
  `ReadyForLockedReproof`. **"Settles" here means the proof result only.** Every
  arm must snapshot the record bytes and the directory entries before and after
  and assert both unchanged. Its truth table already exists in substance, so do
  **not** contort the API to force it red — say plainly in the handoff that it is
  regression coverage and name the two tests above as the genuinely red evidence.

For each red test, record the exact pre-change failure output in the handoff.

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

1. Population projection admits **only**
   `PreservationUnknown { reason: MaterializationInFlight }` to subject
   construction and probing; every other state refuses with an explicit reason
   **before** a candidate is constructed and **before** the probe is called.
2. Bare `ProtectionPrepared` residue refuses as `CannotConstructSubject`; no
   preparation-journal change and no durable-schema work is included.
3. Exact-absence observation and unused-candidate assessment are separate types,
   and there remains exactly **one** exact-absence proof definition.
4. The positive result is named `ReadyForLockedReproof` (or an equally explicit
   non-authority name) and its doc comment states that it is advisory and that
   T3b must re-prove under its lock.
5. `LocallyOwned` and `OwnershipCannotProve` exist in the vocabulary, are
   documented as slice-2 wiring, and are constructed nowhere in this slice.
6. **Zero mutation.** No custody record, marker, sidecar, or checkout directory is
   written, renamed, or unlinked on any path this slice touches. The named exit
   gate asserts unchanged bytes and unchanged directory entries in every arm.
7. No new edge in the frozen custody transition table; no new
   `ScannedWorktreeRecordV1` variant; no async exact-absence trait; no nested
   runtime; no change to `compare_path_identities` or to `host_git.rs`'s proof.
8. Legacy sidecar behavior is unchanged.
9. Both genuinely-red tests exist and their exact pre-change failure output is
   recorded; the named exit gate exists and is honestly labelled regression
   coverage.
10. `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
    --locked -- -D warnings` clean; `cargo test --workspace --locked
    --no-fail-fast` green with totals reported. Report totals as the count of test
    binaries and doc-test suites, not by summing `test result:` lines — a
    bridge-core test re-executes the test binary as a filtered subprocess and its
    nested harness line inflates a naive sum.
11. `git diff --numstat 9aedf175..HEAD` at most **250** changed lines including
    tests and handoff. A breach requires an explicit pre-closure operator waiver.

## Files

- `crates/bridge-worktree/src/sweep.rs` — the vocabulary, the population projection, the split, and the tests.
- `crates/bridge-worktree/src/custody.rs` — read for the state and reason enums; **do not modify**.
- `crates/bridge-worktree/src/host_git.rs` — the probe implementation; read only.

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
```

---

## Acceptance Criteria

A useful review must:

1. **Rule APPROVE or REJECT**, with every blocking objection enumerated. For each,
   name the instruction at fault and what an implementer would do wrong because of
   it. If the only findings are improvements, say APPROVE and list them as
   non-blocking — manufacturing a blocker to avoid approving is itself a failure
   mode here.
2. **Check every factual claim against the code in this checkout.** The spec names
   symbols, states and behaviours in `sweep.rs`, `custody.rs` and `host_git.rs`.
   Say which you verified and which you could not. A spec whose central premise is
   wrong must be rejected regardless of how well written it is — verify
   specifically that `decide_unused_custody_record` really does lack a
   `record.state` gate, and that `PreservationUnknown { reason:
   MaterializationInFlight }` and `ProtectionPrepared {}` are the real variant
   spellings.
3. **Judge the red-first battery honestly.** For each test the spec mandates, say
   whether it would actually be red on unmodified `9aedf175`, and what production
   mutation it would catch. The spec deliberately labels one test as regression
   coverage rather than red — say whether that labelling is correct or whether the
   test is in fact red, and whether the two tests it does claim as red really are.
4. **Attack effect-freedom.** The slice claims no record, marker, sidecar or
   checkout directory is written, renamed or unlinked on any touched path. Say
   whether the instructions actually guarantee that, or whether an implementer
   following them could introduce a write.
5. **Check the fail-open direction.** Every change should narrow authorization or
   leave it unchanged. Flag anything that could widen a positive result, and any
   place where refusing is described where the code would in fact authorize.
6. **Check sizing and completeness.** 250 changed lines including tests and
   handoff — is that achievable for what is specified, or is the spec setting up a
   cap breach? Is anything required by the acceptance criteria not actually
   specified in the body, or vice versa?
7. **Flag anything under-specified enough to produce divergent implementations**,
   especially the population projection's placement relative to candidate
   construction and probing, and the treatment of the vocabulary variants the
   spec says must exist but must not be constructed.

Tag findings **BLOCKER** or **NON-BLOCKING**. A finding without a concrete
consequence for the implementer is non-blocking.

## Files

- `crates/bridge-worktree/src/sweep.rs` — the file the slice changes; contains the population adapters, the exact-absence types, and `decide_unused_candidate`.
- `crates/bridge-worktree/src/custody.rs` — the frozen state machine and `PreservationReasonV1`; the slice must not modify it.
- `crates/bridge-worktree/src/host_git.rs` — the probe implementation the slice must not change.
- `crates/bridge-core/src/fs_custody.rs` — the landed path-identity primitive, out of scope for the slice.

## Spec Refs

The design this slice implements is `docs/superpowers/plans/2026-08-18-r2f1b-3d-t3a-rebuild-design.md`,
which lives on a planning branch and is **not in this checkout**. Its relevant
conclusions are reproduced in the spec above; its absence is not a missing input.
