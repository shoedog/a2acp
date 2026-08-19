I’m using the `prism-nav` skill because the merge turns on exact call/effect boundaries and caller ownership. I’ll keep this strictly read-only: a brief memory lookup plus directed source reads only where the two designs disagree, then I’ll output the merged design and stop.The Prism navigation surface is not callable here, so the skill’s directed-read fallback applies: literal symbol discovery, then only the definition/caller regions needed to resolve the architects’ disagreements. No gates or mutations will be run.## Prior-round adjudication

All named prior defects remain **OPEN** at `9aedf175`; none is fixed in the checkout:

- **F1:** `sweep_orphans_with_exact_absence` still returns `()`.
- **F2:** the typed assessment vocabulary does not exist, so tests using it cannot provide behavioral-red evidence on this base.
- **Ownership:** production passes `recovery_owned = false`; the real inventories are private backend state constructed after the boot sweeps.
- **Guards:** under-root and sibling failures remain collapsed into `Refused`, with no typed precedence.
- **False-green fixtures:** invalid authority can still prevent a guard fixture from reaching the probe independently.
- **Effect freedom:** no bounded transitive audit currently proves that the decision traversal cannot reach mutation.
- **Population wording:** the code remains state-blind; only constructible, admitted claim-bearing records should attempt proof.

## Convergent design

Both architects independently converged on the same spine:

1. Land a total typed reporting seam.
2. Move population and guard policy into exhaustive matches.
3. Bind source, sweep-root, and common-directory authority into the exact-absence subject and revalidate it around Host Git.
4. Keep T3a observational and effect-free; preserve `sweep_orphans`’ independent second scan.
5. Put completeness products in table-driven tests and let the compiler enforce closed enums.
6. Make the work independently landable in three bounded increments.

No custody state, legal transition, record publication, settlement, deletion, or CLI call-site change belongs in this work.

## Components and boundaries

- [sweep.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/crates/bridge-worktree/src/sweep.rs:22) owns checked scanning, guards, population admission, typed subject construction, report construction, and decision projection.
- [host_git.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/crates/bridge-worktree/src/host_git.rs:201) owns every Git observation used by exact absence and both authority brackets.
- `bridge-core/fs_custody.rs` may receive one minimal descriptor-relative child-enumeration helper if `PinnedDirectoryV1` cannot expose the exact enumerated child name safely.
- [custody.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold/crates/bridge-worktree/src/custody.rs:598), `custody_writer.rs`, `backend.rs`, and the five CLI callers remain behaviorally unchanged.
- `sweep_orphans` must continue to discard the T3a report and then perform its existing independent legacy-removal/V3-classification scan. Report entries never become inputs to that destructive path.

## Reporting seam

Land this before admission:

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

Fields remain private with read-only accessors. `ExactAbsenceRecordAssessmentV1::decision()` exhaustively projects unreadable, ineligible, and unconstructible cases to `Refused`; `Legacy` and `Assessed` return their contained decision.

The production signature becomes:

```rust
pub fn sweep_orphans_with_exact_absence(
    root: &str,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactAbsenceSweepReportV1;
```

Production logs the report’s projected decisions and discards it. Tests inspect the typed entries and scan status.

Rejected alternatives:

- Logging-only output does not answer F1.
- A `cfg(test)` collector does not exercise production traversal.
- A callback/closure collector creates a test-shaped production API.
- Landing report and admission together is larger than necessary and forfeits the clean F2 solution: vocabulary first, behavioral change second.

## Scan and guard flow

The checked scanner returns records, enumeration completeness, the canonical root, the pinned root identity, and the exact descriptor-enumerated child name. The existing `scan_worktree_records(root) -> Vec<_>` remains a compatibility wrapper that deliberately erases status for legacy consumers.

For each readable custody record, precedence is:

1. `RecordedWorktreePathNotAbsolute`
2. `OutsideSweepRoot`
3. `RecordFileNotExpectedSibling`
4. Population admission
5. Typed claim/authority construction
6. Exact-absence observation

The sibling guard compares the exact enumerated child name with the lexically derived `<target-name>.custody.v1.json`. It must not canonicalize the expected filename: current canonical equality accepts a wrong regular record when the expected sibling is a symlink back to it.

Isolation fixtures:

- **Outside-root:** use an exact sibling for an in-root target spelling that resolves through a symlink outside the frozen root. The sibling check would pass, so only containment refuses.
- **Wrong sibling:** use an ordinary in-root target with complete valid claim authority, but place the record under another custody filename. Containment passes; exact child comparison refuses.
- Each fixture gets a corrected control that reaches both authority observation and exact-absence probing once. This prevents zero-call tests from passing merely because synthetic source/common-directory authority failed first.

## Population admission

Use one exhaustive match over `WorktreeCustodyStateV1`, with every `PreservationReasonV1` named and no wildcard:

| Canonically decodable population | Result |
|---|---|
| `ProtectionPrepared`, no claim | `IneligiblePopulation(BareProtectionPrepared)` |
| `ProtectionPrepared`, claim | Continue to construction |
| `PreservationUnknown(MaterializationInFlight)`, claim | Continue to construction |
| Other five `PreservationUnknown` reasons, claim | `StateNotCandidate` |
| `PreservationPrepared` or `Preserved`, claim | `StateNotCandidate` |
| Every claim-forbidden state, no claim | `StateNotCandidate` |

Bare `ProtectionPrepared` is not `MissingClaim`: its claim is schema-optional. Claim-bearing `ProtectionPrepared` remains admitted because the durable schema allows it, but its regression is explicitly schema-boundary evidence—the present writer publishes it without a claim.

Invalid required/forbidden claim pairs never become an assessment. The canonical decoder already rejects them, so they remain `UnreadableCustody(Decode(...))`; do not add a dormant `InvalidStateClaimPair` arm.

## Typed subject and Host Git boundary

Use a closed object/reason product:

```rust
pub enum ClaimIdentityFieldV1 {
    Source,
    Root,
    Worktree,
    CommonDirectory,
}

pub enum ClaimIdentityFailureV1 {
    PathMismatch,
    NotAbsolute,
    Degraded,
    Unavailable,
    Changed,
}

pub enum CannotConstructSubjectV1 {
    RecordedWorktreePathNotAbsolute,
    OutsideSweepRoot,
    RecordFileNotExpectedSibling,
    ClaimIdentity {
        field: ClaimIdentityFieldV1,
        failure: ClaimIdentityFailureV1,
    },
    SourceCommonDirectoryMismatch,
    ClaimAuthorityUnproven,
}
```

Construction precedence is:

1. Outer/embedded path agreement for source, root, worktree, common directory.
2. Absolute paths in that same order.
3. Complete identities for source, root, common directory, with first failure winning.
4. Claim root matches the pinned sweep-root object.
5. Observe live source/common-directory authority.
6. Compare the observation against the claim and prove source ownership of the common directory.
7. Construct the candidate.

Worktree identity may be degraded or historical-complete because the target is expected to be absent.

All Git moves behind the injected port:

```rust
pub trait ExactAbsenceProbeV1: Send + Sync {
    fn observe_repository_authority(
        &self,
        source: &str,
    ) -> Result<ObservedRepositoryAuthorityV1, BridgeError>;

    fn observe_exact_absence(
        &self,
        candidate: &ExactAbsenceCandidateV1,
    ) -> Result<ExactAbsenceObservationV1, BridgeError>;
}
```

This is stronger than leaving `git rev-parse` in `sweep.rs`: current candidate construction executes Git directly, so the existing probe does not control or record the whole external observation boundary.

The candidate retains:

```rust
pub struct ExactAbsenceCandidateV1 {
    canonical_source: String,
    source_identity: DirectoryIdentityV1,
    common_dir: String,
    common_dir_identity: DirectoryIdentityV1,
    custody_root_identity: Option<DirectoryIdentityV1>,
    worktree_path: String,
}
```

Legacy candidates use `None`; custody candidates carry the matched root identity.

`HostGitWorktree::observe_exact_absence` performs:

1. Revalidate source, optional custody root, common directory, and Git ownership.
2. No-follow target-presence observation.
3. `git worktree list --porcelain -z`.
4. Repeat the full authority revalidation.
5. Repeat no-follow target observation.
6. Return `TargetPresent`, `RegisteredButAbsent`, or `BothAbsent`.

This remains a bracketed proof, not a transaction. T3b must reread and re-prove under its action lock.

## Degraded-identity matrix

The Cartesian product belongs in one table-driven test; runtime matching supplies typed precedence.

| Population | Rows | Current behavior | Final behavior |
|---|---:|---|---|
| Source degraded | 8 | Refuses before probe | `ClaimIdentity(Source, Degraded)` |
| Source complete, root degraded | 4 | Root is ignored; common-degraded rows refuse, common-complete rows probe | `ClaimIdentity(Root, Degraded)` |
| Source/root complete, common directory degraded | 2 | Refuses before probe | `ClaimIdentity(CommonDirectory, Degraded)` |
| Required authorities complete, worktree degraded | 1 | Probes | Probes |
| All complete | 1 | Probes | Probes |

Add separate non-Cartesian rows for every outer/inner path mismatch, every non-absolute field, and replacement of source, root, or common-directory objects. The historical-complete worktree control captures identity, publishes the record, removes the target, and then expects one probe.

## Effect-freedom evidence

The principal evidence is a bounded transitive source audit from:

- `sweep_orphans_with_exact_absence`
- checked scanning
- guards and admission
- typed subject construction
- both `ExactAbsenceProbeV1` methods
- `HostGitWorktree::observe_exact_absence`

Allowed leaves are bounded reads/decoding, descriptor and metadata observations, canonicalization/identity checks, `git rev-parse`, `git worktree list --porcelain -z`, allocation, collection, and tracing.

The audit must show no edge to provider removal/pruning, `remove_dir_all`, unlink/rename, custody publication/replacement, settlement, transitions, or backend cleanup. Byte snapshots and recording probes remain corroborating regressions; they are not the sole proof because mutation-and-restoration could fool final-state snapshots.

## Smallest shippable slices

### Increment 1 — total report and truthful scan status

Hard cap: **300 changed lines including tests**.

- Introduce the final reporting vocabulary, excluding invalid-pair and ownership arms.
- Add checked scan status and preserve `scan_worktree_records` as a compatibility wrapper.
- Return the report while preserving existing decisions, logging projection, and the later independent scan.

This is intentionally behavior-preserving. It is worth landing alone because it directly closes F1 and makes F2 solvable.

There is **no genuine behavioral-red test**. Its exit evidence is structural compilation, characterization that existing fixtures retain identical decision projections, truthful root/enumeration status tests, the line cap, and all repository gates.

### Increment 2 — guards and exhaustive population admission

Hard cap: **450 changed lines including tests**.

- Implement typed guard precedence and exact child-name matching.
- Implement the exhaustive 16-population state/reason match.
- Populate `IneligiblePopulation` and guard-related `CannotConstructSubject` arms.

Behavioral-red evidence against Increment 1:

- Canonical `Preserved + complete claim + BothAbsent` changes from `Assessed(Authorized)` to `StateNotCandidate`, with zero probe calls.
- Bare `ProtectionPrepared` becomes `BareProtectionPrepared`.
- The expected-sibling symlink alias is distinctly refused.
- Claim-bearing `ProtectionPrepared` and materialization-in-flight unknown reach the recording probe once.
- Invalid persisted claim pairs remain unreadable rather than becoming constructed assessments.

### Increment 3 — retained authority and root-bracketed Host Git

Hard cap: **600 changed lines including tests**.

- Replace string-valued claim-construction errors with the closed typed mapping.
- Move all Git authority observation behind `ExactAbsenceProbeV1`.
- Retain and bracket the custody-root identity.
- Add the 16-row degraded matrix, stale-object rows, and real persisted-record Host Git tests.

Behavioral-red evidence against Increment 2:

- Degraded root currently reaches the probe when source/common directory are valid; it must become `ClaimIdentity(Root, Degraded)`.
- Root replacement during Git can currently yield `BothAbsent` for the replacement root; it must refuse.
- Wrong-repository/common-directory substitution must refuse.
- Degraded and historical-complete worktrees with complete required authority must still reach the probe.
- Every regression asserts unchanged custody bytes.

Each increment must meet its numstat cap, formatting, clippy, build, and `CARGO_INCREMENTAL=0 cargo test --workspace`, with totals reported separately. A projected cap breach is split before implementation, never excused after review.

## Risks

- The proof is still bracketed rather than descriptor-transactional; T3b must revalidate under its action lock.
- The public return-type change can break external function-pointer consumers, although ordinary semicolon-discarding callers remain source-compatible.
- The legacy scanner wrapper intentionally erases completeness; new authorization-sensitive code must use the checked report.
- Tests using only fake probes are insufficient for source/common-directory ownership; retain real two-repository controls.

## DECISIONS FOR THE OWNER

1. **Preparation-flight evidence boundary**

   - **Option A — defer entirely to T3b (recommended):** T3a exposes no ownership input or variants. T3b must consult both active flights—including `TransferPublishing`—and transferred recovery flights, reread durable evidence, and re-prove exact absence under its action lock.
   - **Option B — add a fourth T3a journal-evidence slice:** extract a bounded canonical no-follow preparation-journal reader and require `Clear → BothAbsent → Clear`; treat `Open`, `BarrierSynced`, and `Transferred` as outstanding.

   Recommendation: **Option A**. The production boot sweeps occur before `WorktreeBackend` exists, so T3a cannot truthfully construct in-memory ownership, and deferral yields the smallest coherent boundary. If the owner requires boot-time outstanding-journal refusal, choose Option B explicitly rather than calling it ownership.

**Readiness: ready to plan on the recommended three-increment boundary; no concrete blocker remains.**

