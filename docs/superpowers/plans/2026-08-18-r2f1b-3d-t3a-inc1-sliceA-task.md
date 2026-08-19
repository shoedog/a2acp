---
task-type: implement
---

# R2f1b 3d T3a increment 1, slice A — public shape, projections, and a compatibility-backed report

## Description

Base: `main` = `9aedf175`.

This is **slice A of two**. It lands the complete public reporting vocabulary, the
raw/effective projections, a compatibility-backed report, an injectable scanner
seam, and the characterization matrix. **Slice B** (separately specified) later adds
descriptor enumeration, pinned-root classification, platform gating, and real-Git
authority evidence.

**Slice A is deliberately behavior-preserving.** It changes no decision, no
admission rule, and no refusal. Root authority truthfully reports `Unavailable`
throughout, because slice A builds no root observation set — that is correct, not a
gap.

**T3a decides; T3b acts.** No path here writes, renames, or unlinks. No custody
state, transition, publication, settlement, deletion, or CLI call-site behavior
changes.

### Why the shape is what it is

Two structural facts drive it:

- `sweep_orphans_with_exact_absence` returns `()` and only logs, so nothing can
  assert a typed assessment through the production traversal.
- A test naming vocabulary that does not yet exist fails to **compile**, not to
  fail, and a compile error is not behavioral evidence. Vocabulary must therefore
  land before the increment that changes behavior.

Hence: vocabulary and characterization now, behavioral change later.

### Settled, not open to revision

- **No NEW ownership** input, variant, or plumbing. `decide_unused_candidate` keeps
  its existing `recovery_owned: bool` parameter and its two `false` call sites
  **exactly as they are**. Removing it would break behavior preservation. Ownership
  defers wholly to T3b.
- Increment 2 implements the admission table and guards; increment 3 does retained
  authority. **Do not pull their work forward** — but the public shapes they need
  are frozen here so neither needs a breaking API change.
- No new `bridge-core` surface. No `libc`, no `fdopendir`, no platform gating in
  slice A — all of that is slice B.

## What to build

All in `crates/bridge-worktree/src/sweep.rs`.

### 1. Public vocabulary

`ExactAbsenceSweepReportV1` with **private** fields:

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

The remaining public vocabulary, all of which must exist after slice A:

`ExactAbsenceScanStatusV1`, `ExactAbsenceEnumerationV1`, `ExactAbsenceRootRefusalV1`,
`CustodyRootObservationV1`, `ExactAbsenceSweepEntryV1`,
`ExactAbsenceRecordAssessmentV1`, `CustodyRecordAssessmentV1`,
`CustodyExactAbsenceAssessmentV1`, `IneligiblePopulationV1`,
`CannotConstructSubjectV1`, `ClaimAuthorityUnavailableV1`, `ClaimAuthorityObjectV1`,
`ClaimAuthorityUnavailableReasonV1`, `CustodyStateSnapshotV1`.

**Privacy, in a form Rust can express.** Public *structs* retain private fields with
read-only accessors. Public *enums* use ordinary public payloads — a public enum's
variant fields cannot be private, so do not attempt it. `CustodyRecordAssessmentV1`
provides the private-field evolution point for the custody state/assessment pair.

Each public entry retains:

- `record_path: String`, for display-compatible logs;
- `enumerated_name: OsString`, for exact identity — accessor returns `&OsStr`.
  **Not** a `String`: a lossy conversion would corrupt a non-UTF-8 name and make a
  later guard compare a different name than the one on disk. Future guards must use
  this, never a name recreated from the display string;
- its typed assessment.

`CustodyStateSnapshotV1` contains:

```rust
kind: WorktreeCustodyStateKindV1,
preservation_reason: Option<PreservationReasonV1>,
```

Its conversion **exhaustively matches all ten custody states**. Only
`PreservationUnknown` retains a reason; `RecoveredLive` retains neither the
predecessor digest nor the whole record.

`CannotConstructSubjectV1::ClaimAuthorityUnavailable` carries
`ClaimAuthorityUnavailableV1`, whose private `object` and `reason` fields have
accessors. `ClaimAuthorityObjectV1` and `ClaimAuthorityUnavailableReasonV1` are
`#[non_exhaustive]`, so increment 3 can add detail without changing the enclosing
variant's shape.

**Production constructs only** `Legacy`, `UnreadableCustody`, and
`Custody(… Assessed(…))`. The increment-2 arms stay public and test-constructible
but are not production-constructed here. Comment each with the increment that
populates it. Do not remove them, `cfg(test)`-gate them, or fake-construct one.

### 2. Raw and effective projection

`ExactAbsenceRecordAssessmentV1::decision()` — exhaustive, no wildcard:

- `Legacy(d)` ⇒ `d`
- Custody `Assessed(d)` ⇒ `d`
- unreadable, ineligible, and cannot-construct ⇒ `Refused`

`effective_decision_at` is **stricter**:

```text
out-of-range                                -> None
enumeration not Complete                    -> Some(Refused)
custody root not Pinned                     -> Some(Refused)
Complete + Pinned + Legacy                  -> Some(Refused)
Complete + Pinned + non-Legacy assessment   -> Some(raw decision)
```

Rationale to carry in doc comments: an incomplete scan may have skipped a
conflicting sibling record, so it is not future action authority; `Pinned` binds
enumeration and descriptor-relative custody reads but **not** legacy bytes reopened
through `read_sidecar`; and raw legacy decisions plus log output stay unchanged.

**In slice A, root authority is always `Unavailable`,** so `effective_decision_at`
returns `Some(Refused)` for every row. That is correct and must be stated in the
doc comment and the handoff — slice B is what can produce `Pinned`. Note the
consequence explicitly: **future action code must consume only
`effective_decision_at`, never the raw `decision()`.**

### 3. The scanner seam

Crate-private traits, so slice B can substitute a descriptor implementation without
changing public API:

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

All seam and observation types stay **crate-private**.

Slice A ships **one** implementation: a compatibility source over the existing
`read_dir` plus the existing reads, preserving today's behavior exactly. Its
`finish` returns a `RootObservationSetV1` with no observations, which the classifier
maps to `Unavailable`.

Provide an **injectable** test source so enumeration outcomes are deterministic. A
per-item `ReadDir` error is not reliably constructible on ordinary local
filesystems, so `Incomplete { skipped_entries }` must be tested through injection,
not by trying to provoke a real fault.

Include the pure classifier now, with slice B's contract, even though slice A can
only produce `Unavailable`: `Pinned` only when every required observation exists and
all identities match; `IdentityChanged` when a complete set proves a mismatch;
`Unavailable` when any required identity is absent or unusable.

### 4. Scan flow, slice A

`sweep_orphans_with_exact_absence`:

1. Retains `requested_root` verbatim.
2. Canonicalizes once. On failure ⇒ `Refused(CannotCanonicalize)`, root
   `Unavailable`, no entries.
3. Opens the compatibility source on the **canonical** root. If enumeration cannot
   start ⇒ `Refused(CannotEnumerate)`, no entries.
4. Requests one exact `OsString` name at a time and **processes that record fully,
   emitting its row, before requesting the next name.**
5. Builds the display path from the canonical root using today's lossy conversion,
   and applies the existing display-based selection predicates.
6. Reads legacy entries through the existing path-based `read_sidecar`. **`None`
   remains a silent omission** — no row, no probe, no decision log. This is today's
   behavior and must not change.
7. Reads custody entries with the exact enumerated name, as today.
8. Counts **only iterator-item errors** in `skipped_entries`. A record that fails to
   decode is **not** skipped: it is emitted as `UnreadableCustody(..)`.
9. Returns the report and logs each **raw** `assessment.decision()` through the
   unchanged event shape.

**Enumeration roots differ per entry point and must stay that way.** The
exact-absence sweep enumerates the **canonical** root, as today.
`scan_worktree_records(root)` keeps enumerating the **caller's raw spelling**, keeps
using today's lossy full path for selection and legacy reads, keeps using the exact
`DirEntry::file_name()` for custody reads, keeps flattening iterator errors, keeps
returning `Vec<(String, ScannedWorktreeRecordV1)>`, and discards checked status. It
may share the private machinery **only** if those observable semantics remain
literal.

`sweep_orphans` explicitly discards the typed report via `let _ = …;` and then
performs its existing independent compatibility/action scan, unchanged.
`WorktreeRunEndGuard`, custody locking, classifications, and deletion paths continue
to consume only the compatibility result.

The return type changes from `()`. That is type-compatible for statement-position
callers but **lint-incompatible** under `-D warnings` via `#[must_use]`, and can
also affect explicit unit bindings, unit-returning function pointers,
unit-constrained closures, generic consumers inferring unit, function-body tail
expressions, `if`/`match` branches unified with unit, and macro expression contexts.
The five boot callers are statement-position and need no CLI change beyond the
explicit discard at the internal caller. Record this in the handoff.

## Characterization matrix — concrete expected values

For a readable record whose path guards pass and whose valid complete claim
constructs an `ExactAbsenceCandidateV1`:

| Population | Current raw decision |
|---|---|
| `ProtectionPrepared` with claim | probe mapping |
| `ProtectionPrepared` without claim | `Refused` |
| `PreservationPrepared` with required claim | probe mapping |
| `Preserved` with required claim | probe mapping |
| `PreservationUnknown`, any of six reasons, required claim | probe mapping |
| `UnusedSettled`, `Materializing`, `LiveProtected`, `DeleteAuthorized`, `Removed`, `RecoveredLive` | `Refused` |
| Missing required claim, or forbidden claim present | decode refusal ⇒ `UnreadableCustody`, decision `Refused` |

Probe mapping:

| Probe result | Raw decision |
|---|---|
| `BothAbsent` | `Authorized` |
| `TargetPresent` | `Refused` |
| `RegisteredButAbsent` | `Refused` |
| `Err` | `Refused` |

Guards and legacy:

| Fixture | Result |
|---|---|
| Custody worktree outside sweep root | `Refused`; probe not called |
| Custody record not the expected sibling | `Refused`; probe not called |
| Claim source/common/worktree cannot construct authority | `Refused`; probe not called |
| Undecodable, over-bound, symlinked, directory-shaped, or multiply-linked custody entry | emitted unreadable entry; `Refused` |
| Valid matching in-root legacy sidecar | probe mapping |
| Non-matching or outside-root legacy sidecar | `Refused`; probe not called |
| **Malformed or unreadable legacy sidecar** | **silently omitted; no probe and no decision log** |

**The load-bearing row**: a real persisted `Preserved` custody record with a valid
complete claim, a vanished target, and the probe reporting `BothAbsent` must produce
**raw `Authorized`**. That is today's behavior and it is the fail-open increment 2
closes — pinning it as *currently `Authorized`* is what makes increment 2's change a
genuine behavioral red. Its **effective** decision in slice A is `Refused`, because
root authority is `Unavailable`.

`MultiLink` is asserted only on Unix. Permission-dependent unreadability is
supplementary only; primary tests use deterministic type, symlink, injected-open, or
decode failures.

## Evidence — state it honestly

**There is exactly one base-compatible runtime red, and it is not behavioral:**

```rust
let report = sweep_orphans_with_exact_absence(...);
assert!(std::mem::size_of_val(&report) > 0);
```

It proves only that the return changed from unit to a non-unit report. Include it,
and label it in the handoff as an API-shape assertion, **not** decision-behavior
evidence. Do not manufacture any other red, and never present a compile failure as
red evidence.

Raw behavior is protected by **characterization**. Exhaustive production and
test-side matches give **compiler totality**; the tables give **runtime behavior
evidence**. State those as two separate claims in the handoff.

Seam tests, all deterministic through injection:

- cannot canonicalize;
- cannot enumerate, with zero custody-open calls;
- complete enumeration;
- `Ok, Err, Ok, Err` ⇒ `Incomplete { skipped_entries: 2 }`;
- classifier: equal complete identities ⇒ `Pinned`; unequal complete ⇒
  `IdentityChanged`; any missing ⇒ `Unavailable`;
- iterator incompleteness independent of root classification;
- each row processed before the next `next_name` call;
- malformed legacy omission with zero probe and zero log-helper calls;
- malformed custody inclusion **without** incrementing `skipped_entries`;
- exact non-UTF-8 custody-name identity survives the round trip;
- symlinked-root alias: canonical exact scan versus raw compatibility scan both
  produce today's paths.

### Mutation audit

Audit **only the concrete production path** through
`HostGitWorktree::observe_exact_absence` — `&dyn ExactAbsenceProbeV1` is public and a
downstream implementation could do anything, so no audit can cover every invocation.

Allowed observations and effects: canonicalization; `read_dir` traversal; the
existing **unbounded** legacy `std::fs::read`; bounded custody reads and decoding;
descriptor and metadata observation; allocation and collection; `git rev-parse`;
`git worktree list --porcelain -z`; tracing.

Prove there is **no application edge** from the report traversal to: provider remove
or prune; worktree removal; `remove_dir_all`, unlink, or rename; custody publication
or replacement; settlement or transition; backend cleanup; T3b action.

**Do not call the path globally effect-free** — Git subprocesses and tracing exist,
and a configured tracing sink may write. Byte snapshots are corroborating
final-content evidence only; they cannot exclude a mutate-and-restore.

## Who runs which gate

Your container has no compile loop and cannot produce final gate totals. Do not
fabricate them, and do not treat that as licence to skip them.

- **You**: write the code and tests, state per test whether you executed it, and
  record what your verify stage reports.
- **The operator, on the host**: `cargo fmt --all -- --check`; `cargo clippy
  --workspace --all-targets --locked -- -D warnings`; `CARGO_INCREMENTAL=0 cargo
  test --workspace --locked --no-fail-fast`.
- The handoff carries a **marked operator-evidence section** with a pending
  placeholder per item. Leave the placeholders; the operator fills them.
- **Final numstat and clean-tree status cannot live in the handoff**, because a
  committed handoff cannot attest the state of its own final commit. The operator
  records them in an external receipt keyed to the final SHA. Say so in the handoff
  rather than asserting a numstat you cannot verify.

## On evidence

`bridge-core` compiles for Windows in CI while `liveness` and
`namespace_transaction` are `#[cfg(unix)]`; this lane has lost five landing rounds
to that boundary. Anything unused on non-unix needs
`#[cfg_attr(not(unix), allow(dead_code))]`; the established shape is commit
`790b4191`. State what you gated. Note honestly that pre-existing Unix-only tests
mean you **cannot** claim a green Windows all-target baseline.

Three tests in the adjacent slice looked like evidence and were not; one passed on
macOS/APFS and on this container's overlayfs and failed only on ubuntu/ext4 because
it depended on inode reuse. Prefer deterministic, injection-driven tests over ones
sensitive to filesystem allocation behavior, and name the environment for any that
are not.

**Falsification license.** Every anchor, symbol name and behavioural claim here is
an operator claim measured at `9aedf175`, and the repository is the authority. If a
named symbol does not exist, if `read_sidecar` does not silently omit, if the two
entry points do not enumerate different roots, or if any matrix row is wrong — say
so plainly with the evidence and stop rather than forcing the change to fit. Finding
the work smaller than described is a good outcome. Not open to revision: the
T3a-decides / T3b-acts split, and the exclusion of ownership.

## Acceptance Criteria

1. All fourteen public types exist. Public structs have private fields with
   read-only accessors; public enum variant payloads are ordinary and public.
2. `ExactAbsenceSweepReportV1` exposes accessors, `is_authoritative()`, and
   `effective_decision_at`.
3. Entries retain `record_path: String` **and** `enumerated_name: OsString`, with
   the accessor returning `&OsStr`. No `to_string_lossy()` on the enumerated name.
4. `CustodyStateSnapshotV1` is `{ kind, preservation_reason }`, its conversion
   exhaustively matches all ten states, only `PreservationUnknown` carries a reason,
   and `RecoveredLive` retains no digest.
5. `ClaimAuthorityObjectV1` and `ClaimAuthorityUnavailableReasonV1` are
   `#[non_exhaustive]`; `ClaimAuthorityUnavailableV1` has private fields with
   accessors.
6. `decision()` is exhaustive with no wildcard; `effective_decision_at` implements
   the stated table, and its doc comment records that slice A always yields
   `Some(Refused)` because root authority is `Unavailable`, and that action code
   must use it rather than the raw decision.
7. The crate-private seam traits exist with a compatibility implementation and an
   injectable test implementation; the pure classifier exists with slice B's
   contract.
8. **No decision changes.** The characterization matrix above exists with these
   expected values, including the `Preserved` + valid claim + vanished target +
   `BothAbsent` ⇒ **raw `Authorized`** row and the silently-omitted malformed legacy
   sidecar row.
9. `scan_worktree_records` keeps every listed observable semantic, including
   enumerating the raw spelling; a symlinked-root alias test asserts both entry
   points still produce today's paths.
10. `skipped_entries` counts only iterator-item errors; a decode failure is emitted
    as an entry and does not increment it; each row is emitted before the next name
    is requested.
11. `sweep_orphans` discards the report with an explicit `let _ =` and its
    independent action scan is unchanged.
12. The one API-shape red exists and is labelled as such; no other red is
    manufactured; compiler totality and runtime evidence are stated as separate
    claims.
13. The mutation audit is recorded as a call-path list scoped to production wiring,
    with the allowed-leaf list above, and does **not** claim global effect freedom.
14. Increment-2 arms exist, are documented, and are neither removed,
    `cfg(test)`-gated, nor fake-constructed. No NEW ownership plumbing;
    `decide_unused_candidate` keeps `recovery_owned` and its two `false` call sites.
15. No `libc`, `fdopendir`, descriptor enumeration, root pinning, or platform gating
    — all slice B. No new `bridge-core` surface. No custody state, transition,
    publication, settlement, deletion, or CLI behavior change.
16. The handoff is created from the installed template at
    `~/.claude/handoff-template.md` (resolve it; do not recreate it from memory) at
    `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-inc1-sliceA-handoff.md`, with
    the marked operator-evidence section, and states plainly that slice A has no
    behavioral red and why.
17. Target **at most 600 changed lines** including tests and handoff, measured by
    the operator on a clean committed tree. If your pre-edit estimate exceeds it,
    **say so before implementing** and propose the split — do not compress evidence,
    binding, or handoff work to fit. A breach requires an explicit operator waiver.
18. Report test totals as the count of test binaries plus doc-test suites, not by
    summing `test result:` lines — a bridge-core test re-executes the test binary as
    a filtered subprocess and its nested harness line inflates a naive sum.

## Files

- `crates/bridge-worktree/src/sweep.rs` — all production and test changes.
- `crates/bridge-worktree/src/custody.rs` — read for the state, reason and kind enums; **do not modify**.
- `crates/bridge-worktree/src/host_git.rs` — the probe, read-only, for the mutation audit.
- `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-inc1-sliceA-handoff.md` — the handoff to create.

## Spec Refs

Not in your checkout; their absence is not a missing input, and everything
load-bearing is reproduced above:

- `docs/superpowers/plans/2026-08-18-r2f1b-3d-t3a-increment1-task-v3.md` — the design
  this slice implements, including slice B.

## Commit Message

feat(worktree): return a typed exact-absence sweep report

sweep_orphans_with_exact_absence returned () and only logged each decision, so
nothing could assert a typed assessment through the real production traversal. It
now builds and returns a report — scan status, per-record assessment, an exhaustive
raw projection back to the existing decision, and a stricter effective projection
that refuses whenever the scan was incomplete or the root was not pinned.

Deliberately behavior-preserving. Every raw decision is identical to before, and a
characterization matrix pins them, including the case that matters: a readable
Preserved record with a valid claim and a vanished target still yields Authorized.
That is the fail-open a later increment closes, and pinning it as current behavior
is what will make that change a genuine behavioral red.

The vocabulary includes arms this change never constructs, because the increment
that starts constructing them can only produce behavioral-red evidence if the types
already compile on its base — a test naming a type that does not exist fails to
compile rather than to fail, and a compile error is not behavioral evidence.

Root authority reports Unavailable throughout, so the effective decision is always
Refused. Descriptor enumeration and pinned-root classification are the next slice;
the scanner seam is crate-private so that substitution needs no public API change.
