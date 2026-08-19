I’m using the Prism navigation skill only to adjudicate the two real seam disagreements: legacy-read authority and compatibility/action scan separation. I’ll also do the required lightweight memory pass for this pinned T3a work, using only bounded reads and searches; no edits, builds, tests, or network.The memory pass reinforces one constraint relevant to the merge: only descriptor-bound source/common authority may support future authorization, while legacy path guards remain a separate trust boundary. Prism’s structural MCP is unavailable here, so I’m applying its bounded fallback—literal symbol search followed by direct reads of only the resolved definitions and callers.# Merged design: T3a increment 1 typed exact-absence reporting

Verification basis: bounded read-only inspection of the pinned source seams. No edits, builds, tests, gates, or network operations were performed.

## Prior-round adjudication

### Round 1

1. FIXED — Struct privacy is separated from public enum payload visibility.
2. FIXED — Exact `OsString` entry identity and the scanner boundary are defined.
3. FIXED — Deferred increment-2/3 taxonomy is frozen without a later breaking change.
4. FIXED — The exclusion applies only to new ownership plumbing.
5. FIXED — Canonical exact-sweep and raw compatibility roots remain distinct.
6. FIXED — Enumeration, custody reading, and terminal root observation are bound operationally.
7. FIXED — Streaming order and `skipped_entries` semantics are exact.
8. FIXED — The characterization matrix has literal results.
9. FIXED — The audit is scoped to production domain mutation, not arbitrary probe implementations.
10. FIXED — Iterator and identity failures use injected deterministic seams.
11. FIXED — Compiler exhaustiveness and runtime table coverage are separate evidence.
12. PARTIAL — A credible cap and split trigger exist, but actual size remains unmeasured.
13. FIXED — The full return-type compatibility boundary is documented.

### Round 2

1. FIXED — Malformed legacy sidecars remain silently omitted.
2. FIXED — A retained descriptor session binds enumeration and custody reads; legacy bytes are explicitly not bound.
3. FIXED — The owned streaming scanner API, canonicalization ownership, fallback, and mappings are defined.
4. FIXED — Public types, accessors, and custody-state projection are concrete.
5. FIXED — Claim-authority failure carries an evolvable typed payload.
6. FIXED — State, guard, legacy, and probe products have literal outcomes.
7. FIXED — Equal, unequal, missing-before, and missing-after observations are deterministic tests.
8. FIXED — Existing `read_dir` and unbounded legacy reads are admitted in the audit.
9. FIXED — The report-size assertion is identified as API-shape red, not behavioral red.
10. FIXED — The report-level effective projection fails closed on incomplete or unbound evidence.
11. FIXED — Public entries retain exact names alongside display strings.
12. FIXED — Checked scanner machinery remains crate-private.
13. PARTIAL — The 950-line cap is more credible than 800, but still requires a pre-edit estimate.
14. FIXED — One operative cap replaces stale thresholds.
15. FIXED — Deferred arms are absent only from production construction; tests may construct them.
16. FIXED — All expression contexts affected by the return-type change are documented.

## Convergent spine

Both architects independently chose the same core architecture:

- Keep increment 1 observational and raw-decision-preserving.
- Return a typed report from `sweep_orphans_with_exact_absence`.
- Preserve the destructive/action scan as a separate compatibility path.
- Retain exact filesystem names internally and publicly.
- Use a streaming, injected scanner seam for deterministic status evidence.
- Bind custody reads to retained directory authority and reobserve the named root afterward.
- Preserve malformed-legacy omission and unreadable-custody inclusion.
- Freeze increment-2/3 vocabulary now.
- Keep raw decisions separate from an action-facing effective projection.
- Leave all five boot callers, custody transitions, removal logic, ownership inputs, and Host Git proof semantics unchanged.
- Treat the report-size assertion as API-shape red; characterize all decision behavior.
- Use real repositories and persisted custody records for the load-bearing Host Git case.
- Keep final gates and SHA-bound evidence under operator custody.

## Approach and component boundaries

### `crates/bridge-worktree/src/sweep.rs`

Owns:

- The public report vocabulary and accessors.
- Raw and effective decision projections.
- Exact entry identity.
- Crate-private scanner traits, rows, sessions, and test doubles.
- Compatibility and descriptor-bound scan sources.
- Production report construction and unchanged raw logging.
- Characterization, streaming, identity, alias, non-UTF-8, and projection tests.
- A private logging helper with a test-only thread-local invocation counter.

### `crates/bridge-worktree/Cargo.toml`

Add `libc.workspace = true` for the private streaming `fdopendir`/`readdir` implementation.

### `Cargo.lock`

Update the `bridge-worktree` direct-dependency list. The current package entry does not include `libc`, so omitting this file would make the required `--locked` gates inconsistent.

### Handoff

Create:

`docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-increment1-handoff.md`

It records implementation facts, per-test execution status, pending operator gates, platform exclusions, the production mutation audit, and a pointer to an external final-SHA receipt.

### Read-only evidence files

Do not modify:

- `crates/bridge-worktree/src/custody.rs`
- `crates/bridge-worktree/src/host_git.rs`
- `crates/bridge-core/src/fs_custody.rs`
- `bin/a2a-bridge/src/main.rs`

## Key public interfaces and types

`ExactAbsenceSweepReportV1` contains private:

- `requested_root: String`
- `canonical_root: Option<String>`
- `scan: ExactAbsenceScanStatusV1`
- `entries: Vec<ExactAbsenceSweepEntryV1>`

Expose read-only accessors, `is_authoritative()`, and:

```rust
pub fn effective_decision_at(
    &self,
    index: usize,
) -> Option<UnusedCandidateDecisionV1>;
```

The remaining public vocabulary is:

- `ExactAbsenceScanStatusV1`
- `ExactAbsenceEnumerationV1`
- `ExactAbsenceRootRefusalV1`
- `CustodyRootObservationV1`
- `ExactAbsenceSweepEntryV1`
- `ExactAbsenceRecordAssessmentV1`
- `CustodyRecordAssessmentV1`
- `CustodyExactAbsenceAssessmentV1`
- `IneligiblePopulationV1`
- `CannotConstructSubjectV1`
- `ClaimAuthorityUnavailableV1`
- `ClaimAuthorityObjectV1`
- `ClaimAuthorityUnavailableReasonV1`
- `CustodyStateSnapshotV1`

Public structs retain private fields and read-only accessors. Public enums use ordinary public payloads; `CustodyRecordAssessmentV1` supplies private-field evolution for the custody state/assessment pair.

Each public entry retains:

- `record_path: String` for display-compatible logs.
- `enumerated_name: OsString` for exact identity.
- Its typed assessment.

`enumerated_name()` returns `&OsStr`. Future guards must use it rather than recreating a name from the lossy display string.

`CustodyStateSnapshotV1` contains:

```rust
kind: WorktreeCustodyStateKindV1,
preservation_reason: Option<PreservationReasonV1>,
```

Its conversion exhaustively matches all ten custody states. Only `PreservationUnknown` retains a reason; `RecoveredLive` does not retain the predecessor digest or the complete record.

`CannotConstructSubjectV1::ClaimAuthorityUnavailable` carries `ClaimAuthorityUnavailableV1`, whose private `object` and `reason` fields have accessors. The object and reason enums are `#[non_exhaustive]`, allowing increment 3 to add detail without changing the enclosing V1 variant shape.

Production constructs only:

- `Legacy`
- `UnreadableCustody`
- `Custody(…Assessed(…))`

Increment-2 arms remain public and test-constructible but are not production-constructed here.

## Raw and effective projection

`ExactAbsenceRecordAssessmentV1::decision()` is exhaustive, without a wildcard:

- `Legacy(d)` returns `d`.
- Custody `Assessed(d)` returns `d`.
- Unreadable, ineligible, and cannot-construct assessments return `Refused`.

`effective_decision_at` is stricter:

```text
out-of-range                                      -> None
enumeration not Complete                          -> Some(Refused)
custody root not Pinned                           -> Some(Refused)
Complete + Pinned + Legacy                        -> Some(Refused)
Complete + Pinned + non-Legacy assessment         -> Some(raw decision)
```

This merges the strongest parts of both designs:

- An incomplete scan may have skipped a conflicting sibling record, so it is not future action authority.
- `Pinned` binds enumeration and descriptor-relative custody reads, but not legacy bytes reopened through `read_sidecar`.
- Existing raw legacy decisions and log output remain unchanged.

## Crate-private scanner interface

```rust
trait CheckedScanSourceV1 {
    fn open(
        &self,
        enumeration_root: &Path,
    ) -> Result<Box<dyn CheckedScanRootSessionV1>, CheckedScanOpenRefusalV1>;
}

trait CheckedScanRootSessionV1 {
    fn next_name(
        &mut self,
    ) -> Option<Result<OsString, CheckedScanEntryRefusalV1>>;

    fn read_legacy(
        &self,
        enumerated_name: &OsStr,
        record_display: &str,
    ) -> Option<WorktreeSidecar>;

    fn read_custody(
        &self,
        enumerated_name: &OsStr,
    ) -> Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1>;

    fn finish(self: Box<Self>) -> RootObservationSetV1;
}
```

All seam and observation types remain crate-private. `RootObservationSetV1` carries raw observations for:

- The retained enumeration object.
- The pinned custody directory.
- The final no-follow open of the named root.

A pure classifier yields:

- `Pinned` only when every required observation exists and all three `RequiredObjectIdentityV2` values match.
- `IdentityChanged` when a complete observation set proves a mismatch.
- `Unavailable` when any required identity is absent or unusable.

This uses the repository’s required `(dev, ino, birthtime)` identity. It is the project’s fail-closed identity model, not a claim of absolute immunity to filesystem object-ID reuse.

## Scan flow

### Exact observational scan

`sweep_orphans_with_exact_absence`:

1. Retains `requested_root` verbatim.
2. Canonicalizes once.
3. On failure, returns `CannotCanonicalize`, `Unavailable`, and no entries.
4. Opens the enumeration descriptor before attempting custody pinning.
5. If enumeration cannot start, returns `CannotEnumerate`; custody is not opened.
6. Opens one `PinnedDirectoryV1` for descriptor-relative custody reads.
7. Establishes a streaming descriptor-derived enumeration session.
8. Requests one exact `OsString` name at a time.
9. Constructs the display path using the canonical root and current lossy conversion.
10. Applies the existing display-based selection predicates.
11. Processes that record fully and emits its row before requesting another name.
12. Reads legacy entries through the existing path-based `read_sidecar`; `None` remains silent omission.
13. Reads custody entries through the pinned directory with the exact name.
14. Counts only iterator-item errors in `skipped_entries`.
15. After termination, performs the final named-root observation even if an earlier identity observation failed.
16. Returns the report and logs each raw `assessment.decision()` through the unchanged event shape.

On Linux and macOS, the descriptor stream uses a duplicated descriptor plus `fdopendir`/`readdir`, with explicit errno handling. Other targets use the compatibility source and report root authority as `Unavailable`. No authoritative rows may be emitted before falling back.

### Compatibility/action scan

`scan_worktree_records(root)` continues to:

- Enumerate the caller’s raw spelling.
- Open `read_dir` before attempting `PinnedDirectoryV1`.
- Use the current lossy full path for selection and legacy reads.
- Use exact `DirEntry::file_name()` for custody reads.
- Flatten iterator errors.
- Return the existing `Vec<(String, ScannedWorktreeRecordV1)>`.
- Discard checked status.

It may share the private streaming machinery only if these observable semantics remain literal.

`sweep_orphans` explicitly discards the typed report and then performs its existing independent compatibility/action scan. `WorktreeRunEndGuard`, custody locking, classifications, and deletion paths continue to consume only the compatibility result.

## Concrete characterization matrix

For a readable record whose path guards pass and whose valid complete claim constructs an `ExactAbsenceCandidateV1`:

| Population | Current raw decision |
|---|---|
| `ProtectionPrepared` with claim | Probe mapping |
| `ProtectionPrepared` without claim | `Refused` |
| `PreservationPrepared` with required claim | Probe mapping |
| `Preserved` with required claim | Probe mapping |
| `PreservationUnknown` with any of six reasons and required claim | Probe mapping |
| `UnusedSettled`, `Materializing`, `LiveProtected`, `DeleteAuthorized`, `Removed`, `RecoveredLive` | `Refused` |
| Missing required claim or forbidden claim present | Decode refusal; emitted `UnreadableCustody`, decision `Refused` |

Probe mapping is:

| Probe result | Raw decision |
|---|---|
| `BothAbsent` | `Authorized` |
| `TargetPresent` | `Refused` |
| `RegisteredButAbsent` | `Refused` |
| Error | `Refused` |

Guard and legacy results are:

| Fixture | Result |
|---|---|
| Custody worktree outside sweep root | `Refused`; probe not called |
| Custody record not expected sibling | `Refused`; probe not called |
| Claim source/common/worktree cannot construct authority | `Refused`; probe not called |
| Undecodable, over-bound, symlinked, directory-shaped, or multiply linked custody entry | Emitted unreadable entry; `Refused` |
| Valid matching in-root legacy sidecar | Probe mapping |
| Non-matching or outside-root legacy sidecar | `Refused`; probe not called |
| Malformed or unreadable legacy sidecar | Silently omitted; no probe and no decision log |
| Pinned legacy symlink resolving to valid external bytes | Raw decision may be `Authorized`; effective decision is `Refused` |

The load-bearing positive test uses a real Git repository and persisted `Preserved` custody record with a valid complete claim, vanished target, and `HostGitWorktree` reporting `BothAbsent`. It must produce raw and effective `Authorized`.

A second real-repository test mixes source identity from repository A with common-directory identity from repository B and must refuse before the programmable probe.

## Status and streaming evidence

Deterministic seam tests cover:

- Cannot canonicalize.
- Cannot enumerate, with zero custody-open calls.
- Complete enumeration.
- `Ok, Err, Ok, Err` producing `Incomplete { skipped_entries: 2 }`.
- Equal complete root identities producing `Pinned`.
- Unequal complete identities producing `IdentityChanged`.
- Missing enumerator, custody, or final identity producing `Unavailable`.
- Final observation being attempted after an earlier observation failure.
- Iterator incompleteness remaining independent from raw root classification.
- Each row being processed before the next `next_name` call.
- Malformed legacy omission with zero probe and log-helper calls.
- Malformed custody inclusion without incrementing `skipped_entries`.
- Exact non-UTF-8 custody-name identity.
- Symlinked-root alias behavior for canonical exact scans versus raw compatibility scans.

`MultiLink` is asserted only on Unix. Unsupported-platform custody behavior receives a separate refusal test. Permission-dependent unreadability is supplementary only; primary tests use deterministic type, symlink, injected-open, or decode failures.

## Evidence and compatibility

The only base-compatible runtime red is:

```rust
let report = sweep_orphans_with_exact_absence(...);
assert!(std::mem::size_of_val(&report) > 0);
```

It proves the return changed from unit to a non-unit report. It is not decision-behavior evidence.

Raw behavior is protected by characterization. Exhaustive production and test-side matches provide compiler totality; the tables provide runtime behavior evidence. These are separate claims.

The return-type change can affect:

- `#[must_use]` statement calls under `-D warnings`.
- Explicit unit bindings.
- Unit-returning function pointers.
- Closures or callbacks constrained to unit.
- Generic consumers whose output is inferred as unit.
- Function-body tail expressions.
- `if` or `match` branches unified with unit.
- Macro-generated expression contexts.

The five known boot callers remain statement-position calls and require no CLI behavior change beyond explicit discard at the internal caller.

## Production domain-mutation audit

Audit only the concrete production path through `HostGitWorktree::observe_exact_absence`.

Allowed observations/effects include:

- Canonicalization.
- `read_dir` or descriptor traversal.
- Existing unbounded legacy `std::fs::read`.
- Bounded custody reads and decoding.
- Descriptor and metadata observation.
- Allocation and collection.
- `git rev-parse`.
- `git worktree list --porcelain -z`.
- Tracing.

Prove there is no application edge from the report traversal to:

- Provider remove or prune.
- Worktree removal.
- `remove_dir_all`, unlink, or rename.
- Custody publication or replacement.
- Settlement or transition.
- Backend cleanup.
- T3b action.

Do not call the path globally effect-free: Git subprocesses and tracing exist, and a configured tracing sink may write. Byte snapshots are corroborating final-content evidence only.

## Risks

- The raw/effective distinction is easy to misuse; future action code must consume only `effective_decision_at`.
- Direct `fdopendir`/`readdir` code requires careful ownership, errno, and drop handling.
- Required birthtime may make root authority unavailable on some filesystems.
- The project identity tuple is not an absolute filesystem generation identifier.
- Existing Unix-only tests prevent an honest claim that the pre-existing Windows all-target baseline is green.
- The full matrix, real-Git fixture, public accessors, and handoff may exceed even 950 changed lines.
- A committed handoff cannot itself attest the clean status of its own final commit; final status and numstat must live in an external receipt keyed to the final SHA.
- The required installed handoff template must be resolved before implementation rather than recreated from memory.

## Smallest shippable slices and build order

1. Preflight: resolve the handoff template, estimate changed lines, and decide one increment versus stacked slices.
2. Public vocabulary: report, exact entry identity, state snapshot, claim-authority payload, raw/effective projections, and API-shape red.
3. Scanner seam: owned streaming session, compatibility source, injected iterator/identity tests, and unchanged compatibility-wrapper behavior.
4. Descriptor source: retained enumeration object, pinned custody reader, final root observation, platform gates, `libc` dependency, and lockfile.
5. Production wiring: canonical exact report, raw logging helper, explicit discard, and unchanged action scan.
6. Characterization: full state/probe/guard tables, legacy omission, real Git positive and wrong-repository controls.
7. Closure: mutation audit, source-compatibility notes, operator placeholders, and external SHA-receipt contract.

If the pre-edit estimate exceeds 950 additions plus deletions, use two stacked review slices:

- Slice A: complete public shape, raw/effective projections, compatibility-backed report, injected seam, and characterization. Root authority may truthfully remain `Unavailable`.
- Slice B: descriptor enumeration, pinned root classification, platform integration, and real-Git authority evidence.

No evidence, binding, or handoff work may be compressed merely to fit the cap.

## DECISIONS FOR THE OWNER

1. Delivery size:

   - Option A: one hard-capped 950-line increment with a mandatory midpoint size check.
   - Option B: two stacked, independently reviewed slices as defined above.

   Recommendation: choose Option B unless a pre-edit accounting demonstrates credible margin below 950 lines. Both independent designs found the single-slice budget tight, and prior cap failures show that zero-margin budgeting degrades evidence first.

Ready to plan after the owner selects the one-slice or stacked delivery policy; no concrete correctness blocker remains.

