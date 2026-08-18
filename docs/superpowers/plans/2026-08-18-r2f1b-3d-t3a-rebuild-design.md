I’m merging the two designs around their strong common spine and will use read-only inspection only where they materially disagree: whether T3a may introduce durable preparation evidence, what the two eligible populations actually are, and whether the named exit gate belongs in T3a. I’m also using the Prism navigation skill’s bounded fallback for those ownership/call-graph seams; no edits, builds, tests, or network calls.The first disagreement resolves at the contract boundary, not by preference: the structure lens’s preparation-journal v2 would add a new durable write and a crash-recovery protocol, while the supplied T3a charter says “no record mutation whatsoever” and explicitly excludes T3b’s action window. I’m checking the live schemas and historical split only to decide whether that durable work is truly necessary for T3a’s typed advisory result or belongs to the broader slice/T3b backlog.The live code confirms the narrower synthesis: `ProtectionPrepared` is published with no claim, while the preparation journal stores only `{flight_id, state}`. That makes post-crash settlement evidence a real broader gap, but solving it inside T3a would violate the explicit decision-only/no-record-write split. The merged design will therefore keep T3a effect-free, make bare crash residue fail closed, and surface durable post-crash recovery as an owner decision/prerequisite—not smuggle it into this slice.## Prior-round adjudication

- **FIXED — shared exact-absence substrate.** `ExactAbsenceCandidateV1` now binds source and Git common-directory identities; `ExactAbsenceProbeV1` is synchronous; only `BothAbsent` is positive and probe errors refuse. Do not rebuild these mechanisms in [sweep.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/crates/bridge-worktree/src/sweep.rs:22).
- **PARTIAL — marker population.** V3 custody records already reach `decide_unused_custody_record`; no new scan variant is needed. However, every claim-bearing state currently reaches the proof, so an externally vanished `Preserved` record can report `Authorized`. Admission must be restricted to `PreservationUnknown(MaterializationInFlight)` before candidate construction or probing ([sweep.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/crates/bridge-worktree/src/sweep.rs:504)).
- **OPEN — recovery ownership coupling.** Both production adapters still pass hardcoded `false`; neither active nor transferred preparation ownership influences the decision.
- **OPEN — named exit gate.** `unused_candidate_settles_only_after_exact_absence` is absent.
- **FIXED — reachability and effect freedom.** The exact-absence pass is called through `sweep_orphans` from five boot paths and only reports decisions. The subsequent legacy sweep is a distinct destructive pass ([sweep.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/crates/bridge-worktree/src/sweep.rs:529)). `WorktreeRunEndGuard::drop` does not run the exact-absence pass; its synchronous-Git comment is accurate and should remain.
- **OPEN, but broader than the current T3a contract — post-crash `ProtectionPrepared` evidence.** That record has no claim, and the companion journal stores only `{flight_id, state}` ([backend.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/crates/bridge-worktree/src/backend.rs:220), [custody_writer.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/crates/bridge-worktree/src/custody_writer.rs:564)). A process crashing after `ProtectionPrepared` therefore leaves insufficient source/common-directory authority for a positive proof. T3a must refuse that residue unless the owner separately authorizes a durable-schema prerequisite.

## Merged approach

The convergent design is a three-layer, effect-free decision pipeline:

1. **Population projection:** decide whether a scanned record belongs to a chartered population.
2. **Exact-absence observation:** use the existing state-agnostic proof for every constructible subject.
3. **Local ownership qualification:** for candidate decisions made with a live backend, conservatively consult active and recovery preparation inventories.

The strongest T3a result should be named `ReadyForLockedReproof` or `ProvedAtObservation`, never `Authorized`. It is an advisory observation, not a durable capability. T3b must later rerun the complete proof within its action window; designing that window or its mutations remains out of scope.

Boot cannot honestly claim global non-ownership. Its backend does not yet exist, and another process may own the same target. Boot should therefore report exact absence only, while the backend-local path provides the ownership-qualified decision seam for T3b.

## Key types and interfaces

In `sweep.rs`:

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

pub enum LocalPreparationOwnershipV1 {
    LocallyOwned,
    NoLocalOwnerObserved,
    CannotProve,
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

Keep one proof definition:

```rust
fn decide_exact_absence(
    candidate: &ExactAbsenceCandidateV1,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactAbsenceDecisionV1;
```

Its table remains:

- `BothAbsent` → `ProvedAtObservation`
- `TargetPresent` → corresponding refusal
- `RegisteredButAbsent` → corresponding refusal
- probe error or ambiguity → `CannotProve`

A second pure function combines that result with an already-observed local ownership value. Do not add an async trait, nested runtime, or another public host-Git abstraction.

## Population flow

### Materialization-in-flight marker

The V3 record adapter must:

1. Validate root containment and custody-record pathname.
2. Admit only `PreservationUnknown { reason: MaterializationInFlight }`.
3. Require a constructible, authority-bound claim.
4. Call the shared exact-absence proof.
5. Return/log the typed advisory decision without changing any bytes.

`Preserved`, `PreservationPrepared`, other unknown reasons, live states, tombstones, and `ProtectionPrepared` without independent authority refuse before the host probe.

A degraded materialization claim—such as a real provider error with plan-derived common-directory identity—remains `CannotConstructSubject`. Do not invent source authority or add a path-only constructor.

Legacy sidecars remain a separate legacy protocol. They may continue using the common exact-absence observation, but they should not be represented as one of the two T3a V3 populations.

### Protection-prepared candidate

A scanned bare `ProtectionPrepared` record cannot currently construct an exact-absence subject. It must refuse.

For live backend ownership coupling, add an immutable key to `ActivePreparationFlightV1` using values already available from `BoundWorktreeCustodyV1` and `ResolvedWorktree`:

```rust
struct PreparationOwnershipKeyV1 {
    custody_id: WorktreeCustodyIdV1,
    checkout_fingerprint: Sha256HexV1,
    worktree_path: String,
}
```

The transferred recovery entry already owns the exact `Arc<ActivePreparationFlightV1>`, so the key moves without reconstruction ([backend.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/crates/bridge-worktree/src/backend.rs:530)).

Do not add a journal v2 in T3a. Durable source/common-directory evidence for post-crash positive settlement is a separate scope decision.

## Ownership observer and deadlock argument

Implement a private synchronous backend observer:

1. `try_lock(preparation_recovery_flights)`.
2. While holding it, `try_lock(preparation_flights)`.
3. Snapshot immutable ownership keys or `Arc`s.
4. Release both mutexes.
5. Compare custody IDs and target identities outside the locks.
6. Return:
   - matching custody ID or any target `Same` → `LocallyOwned`;
   - contention, poison, incomplete key, or any identity `CannotProve` → `CannotProve`;
   - only an all-`Different` snapshot → `NoLocalOwnerObserved`.

All active owners count, not only `TransferPublishing`. That automatically covers the interval before recovery-map insertion.

This matches the established transfer order: recovery is locked before active during the move ([backend.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/crates/bridge-worktree/src/backend.rs:938)). The observer uses `try_lock`, so it never waits while transfer or configure owns a mutex.

No mutex may be held across:

- `await`;
- Git execution;
- filesystem/path-identity comparison;
- exact-absence probing.

A stale post-snapshot result remains possible, which is why the output is advisory and T3b must re-observe later.

## Component boundaries

- `crates/bridge-worktree/src/sweep.rs`
  - refusal vocabulary;
  - population projection;
  - shared exact-absence decision;
  - advisory per-record output;
  - marker tests.
- `crates/bridge-worktree/src/backend.rs`
  - immutable preparation ownership key;
  - active/recovery snapshot;
  - backend-local ownership-qualified assessment;
  - transfer and contention tests.
- `crates/bridge-worktree/src/host_git.rs`
  - reuse the existing implementation; no new proof.
- `bin/a2a-bridge/src/main.rs`
  - retain the five live boot entrances; change wording only if needed to avoid “authorized/unused” claims.
- `custody.rs`, `custody_writer.rs`, `bridge-core`
  - no T3a production changes.

## Risks and controls

- **Wrong-state positive result:** gate state before claim construction and probing.
- **False non-ownership during transfer:** snapshot both maps in recovery→active order and count all active owners.
- **Mutex uncertainty:** contention and poisoning refuse.
- **Path aliasing:** use the landed tri-state identity primitive; different strings or custody IDs alone never prove different targets.
- **Cross-process race:** boot reports only an observation; no result is reusable mutation authority.
- **Degraded claims:** refuse rather than reconstruct authority.
- **Unbounded inventory size:** do not claim a strict snapshot bound; this is operational debt, though it does not create authorization.
- **Platform evidence:** pure decision and mutex tests run everywhere. Real identity-bound candidate and Host Git tests should be `#[cfg(unix)]` and run on both macOS/APFS and Ubuntu/ext4. Preserve the Windows compile gate; do not introduce a new `bridge-core` surface.

## Smallest shippable slices

### Slice 1 — population boundary and advisory vocabulary

Budget: **250 changed lines maximum**, including tests and handoff.

Work:

- Split exact-absence observation from unused-candidate assessment.
- Add explicit refusal reasons.
- Restrict the V3 adapter to `PreservationUnknown(MaterializationInFlight)`.
- Preserve byte-for-byte effect freedom.
- Keep legacy behavior separate.

Tests:

- `only_materialization_inflight_records_enter_unused_marker_proof`
  - Real claim-bearing `Preserved`, `PreservationPrepared`, unrelated unknown, and eligible materialization-in-flight records.
  - Only the eligible record may invoke the probe.
  - This is behaviorally red on the current all-claims routing.
- `unused_candidate_settles_only_after_exact_absence`
  - Real eligible record; target-present, registered-but-absent, probe-error, and both-absent arms.
  - Snapshot record bytes and directory entries for every arm.
  - In T3a, “settles” means the proof result only; no mutation.
- `degraded_materialization_marker_refuses_without_probing`
  - Use the real provider-error writer shape.
  - Require `CannotConstructSubject`, zero probe calls, and unchanged bytes.

Exit: focused tests, full workspace suite with `CARGO_INCREMENTAL=0`, repository hygiene, and two counted review rounds.

### Slice 2 — backend-local ownership coupling

Budget: **350 changed lines maximum**, including tests and handoff.

Work:

- Retain the immutable ownership key on every active owner.
- Preserve it through transfer.
- Add the nonblocking two-map snapshot.
- Add the ownership-qualified assessment wrapper.
- Do not add a caller that pretends boot has a backend inventory.

Tests:

- `recovery_inventory_refuses_active_transfer_and_recovery_owners`
  - Ordinary active owner, transfer-publishing active owner, transferred recovery owner, both mutex-contention cases, identity ambiguity, and no match.
  - Every owned/uncertain case refuses without invoking exact absence.
- `transferred_owner_retains_the_exact_ownership_key`
  - Assert transfer retains the same `Arc` and typed key.
- `no_local_owner_observed_is_only_advisory`
  - Only an all-different inventory may reach exact absence, and its positive result is `ReadyForLockedReproof`, not action authority.

For genuine behavioral red evidence, first introduce a compiling wrapper that mirrors today’s hardcoded-`false` behavior, demonstrate the ownership test failing, then implement the snapshot. Do not count an unresolved-symbol compilation failure as behavioral evidence.

Exit: focused tests, full workspace suite, macOS and Ubuntu identity integration, repository hygiene, and two counted review rounds.

## Do not build

- No `UnusedSettled` publication or custody transition.
- No marker unlink, provider removal, `git worktree remove`, or directory removal.
- No T3b proof-to-action lock window.
- No preparation-journal v2 or durable record digest in T3a.
- No new custody transition-table edge.
- No new scan enum variant.
- No async exact-absence trait or nested runtime.
- No Git/filesystem work while backend mutexes are held.
- No exact-absence invocation from `WorktreeRunEndGuard::drop`.
- No path-identity changes.
- No merge or rebase of the parked T3a branches.

## DECISIONS FOR THE OWNER

1. **Post-crash `ProtectionPrepared` completeness**
   - Option A: T3a refuses bare records; durable candidate evidence becomes a separately chartered prerequisite before T3b.
   - Option B: amend T3a to add preparation-journal v2 with source/common-directory authority.
   - **Recommendation: A.** Option B contradicts the explicit no-record-mutation split and materially enlarges the slice.

2. **Meaning of “pre-target marker”**
   - Option A: T3a assesses every `PreservationUnknown(MaterializationInFlight)` record but labels the result advisory; T3b receives a separate ruling before acting.
   - Option B: require proof that the original add outcome was `ProvablyAbsent`, which needs new durable producer evidence.
   - **Recommendation: A for T3a only.** Do not infer original pre-target status or grant removal authority from the state name.

3. **Literal red-first requirement for the named gate**
   - Option A: accept the named four-arm test as regression/exit coverage because its pure truth table already exists, while requiring separate genuinely red wrong-population and ownership tests.
   - Option B: change vocabulary/API solely so the named test itself fails on main.
   - **Recommendation: A.** Manufacturing a red result would weaken evidence discipline.

**Ready to plan after deciding the three open scope questions; there is no concrete fail-open blocker while T3a remains strictly effect-free.**

