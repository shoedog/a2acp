---
task-type: design
---

# Author the corrected T3a increment 1 implement task spec

## Description

**Your output is not a design document. It is a complete, dispatch-ready
implementation task spec**, in the exact format given under "Required output
format" below. Write the whole file. Do not summarise, do not review, do not
explain what you would change — emit the corrected spec itself.

The repository at the session cwd is checked out at `main` = `9aedf175`, the base
the spec targets, and it is authoritative for every factual claim.

### Why you are writing this instead of reviewing it

You reviewed this spec twice. Round 1 returned 9 blockers and 13 findings; round 2,
after the operator folded all 13, returned 9 blockers and 16 findings. Blockers
flat, findings up, kinds repeating — and two round-2 blockers were defects the
operator's own round-1 fixes introduced while transcribing your findings into
prose.

The operator has authorised **one extension past the declared 2-round cap**, on the
condition that the author changes. The transcription step is the failure, so you
now write the spec directly and the operator reviews it.

### What the increment is, and what must not change about it

This is increment 1 of three rebuilding T3a's residual. It is **deliberately
behavior-preserving**: it lands a typed reporting seam and a truthful scan status,
and changes no decision, admission rule, or refusal.

It exists to remove two structural blockers you identified:

- `sweep_orphans_with_exact_absence` returns `()` and only logs, so nothing can
  assert a typed assessment through the production traversal.
- A test naming vocabulary that does not yet exist fails to **compile** rather than
  to fail, and a compile error is inadmissible as behavioral evidence — so
  vocabulary must land before the increment that changes behavior.

Constraints that are settled and **not open to revision**:

- **T3a decides; T3b acts.** No write, rename, or unlink on any path.
- **No NEW ownership** input, variant, or plumbing. `decide_unused_candidate` keeps
  its existing `recovery_owned` parameter and its two `false` call sites untouched.
  Ownership defers wholly to T3b, because all five production boot sweeps run at
  the top of command entry points while `WorktreeBackend` is built in a per-run
  session factory.
- Increment 2 implements the admission table and the guards; increment 3 does
  retained authority and root-bracketed Host Git. **Do not pull their work
  forward** — but *do* freeze whatever public shapes they need, so neither requires
  a breaking API change.
- No custody state, transition, publication, settlement, deletion, or CLI
  call-site behavior change; no new `bridge-core` surface.

### Your round-2 findings, which the new spec must resolve

Resolve all sixteen. Where a finding asks for a literal type, signature, mapping or
matrix, **put the literal thing in the spec** — that is the whole point of you
authoring it. Two are adjudicated:

- **Finding 9 is conceded on the letter.** Binding the result and asserting
  `size_of_val(&report) > 0` does fail on base. The spec must state that honestly
  rather than claiming no base-red exists — while making clear it is a type-shape
  assertion, not behavioral evidence, and that no *behavioral* red exists for this
  increment.
- **Finding 1 is confirmed and is the operator's error to undo.** Verified in this
  checkout: `read_sidecar` returning `None` means the entry is silently **omitted**
  today. The previous spec required malformed legacy sidecars to appear as entries,
  which would have been a behavior change. Preserve today's omission.

```
ROUND 2 FINDINGS
Prior-round adjudication:
- FIXED — Rust enum-field privacy was corrected to apply only to structs.
- FIXED — ExactAbsenceProbeV1’s one-method shape and production-only effect-freedom scope were corrected.
- PARTIAL — The vacuous “existing tests stay green” claim was replaced by a characterization matrix, but several fixtures and literal expectations remain unspecified.

Disagreements resolved: Soundness is right that a runtime base-red exists for the return-value/API change, though not for decision behavior; Soundness is also right to treat the stale 300-line review criterion as non-gating because task AC16 establishes the operative 500-line cap.

1. BLOCKER — ExactAbsenceEnumerationV1 / legacy parsing — WRONG. A malformed legacy sidecar is currently omitted, but the spec requires it to appear while providing no assessment variant capable of representing it. An implementer must either violate behavior preservation, misclassify the row, or violate the frozen taxonomy. Resolve by preserving omission in increment 1 or adding an explicit unreadable-legacy outcome with defined projection, entry, and logging behavior.

2. BLOCKER — CustodyRootObservationV1 — WRONG. Before/after path observations do not bind the directory actually enumerated or used for custody reads. An A→B→A replacement can therefore report Pinned while combining rows from different root objects; on non-Unix, absent dev/inode identity makes same-name replacement particularly unprovable. Require usable stable object identity, retain one root object for enumeration and record reads, and specify every open, its order, and its descriptor consumer. Otherwise return Unavailable or another explicitly non-authoritative status.

3. BLOCKER — Checked scanner contract — WRONG. Result structs are specified without the scanner signature, canonicalization ownership, refusal mapping, compatibility-wrapper adaptation, or streaming behavior. Implementations can consequently canonicalize the legacy wrapper, duplicate scans, change record paths, or collect names before reading records and alter concurrent-replacement behavior. Specify one exact streaming API and the complete mapping for both canonical-root and raw-root entry points.

4. BLOCKER — Frozen public API — SMELL. CustodyStateSnapshotV1 lacks literal fields, types, accessors, conversion mapping, and required trait surface, while other accessors are also underspecified. Different implementations can expose incompatible APIs that increment 2 cannot consume without revision. Provide complete definitions and accessor signatures for every frozen public type.

5. BLOCKER — Deferred taxonomy — WRONG. ClaimAuthorityUnavailable is frozen as a unit variant while increment 3 is promised a typed object/reason product. Adding that payload later breaks construction and exhaustive matching of the V1 enum. Carry an evolvable private-field payload struct now or explicitly commit increment 3 to a new versioned assessment type.

6. BLOCKER — Characterization matrix — SMELL. Load-bearing rows remain “today’s result,” and their outcomes vary with probe results and claim shape. Two implementers can write materially different matrices while satisfying the prose. Specify each complete fixture, probe result, and literal expected decision, or define the required Cartesian product.

7. BLOCKER — Root-observation evidence — WRONG. The spec permits IdentityChanged and failure arms to remain unexecuted, allowing reversed comparisons or incorrect Pinned mappings to pass. Require an injected observation seam with deterministic coverage of Pinned, IdentityChanged, and both before/after observation failures.

8. BLOCKER — Effect-freedom audit — WRONG. The allowed-leaf list permits only bounded reads, but the current path includes read_dir traversal and legacy std::fs::read without stated bounds. The required audit therefore cannot truthfully satisfy its own whitelist. Explicitly admit these existing read-only leaves or separate mutation-freedom from resource-boundedness.

9. BLOCKER — Red-first battery — WRONG. A genuine runtime base-red exists for the intended API change: bind the production result and assert std::mem::size_of_val(&report) > 0; it compiles on both versions, fails for the current unit return, and passes for the report. Distinguish “no decision-behavior red” from “no API-observability red” and require this regression evidence.

10. MAJOR — Projection / root status — WRONG. entry.assessment().decision() may return Authorized while the enclosing report says IdentityChanged or Unavailable. Freeze a report-level effective-decision projection, or specify the exact increment-2 conjunction and how root failure becomes a refusal without changing the frozen API.

11. MAJOR — Report identity — WRONG. CheckedScanRowV1 retains OsString, but the public entry exposes only a lossy String path, so distinct non-UTF-8 names can become indistinguishable. Retain an exact OsString or PathBuf identity in the report and use String only for display-compatible logging.

12. MAJOR — Checked scanner exposure — SMELL. Public CheckedScanV1 and CheckedScanRowV1 freeze an internal intermediate coupled to ScannedWorktreeRecordV1. Keep the streaming checked scanner crate-private unless an identified external consumer requires that surface.

13. MAJOR — Sizing — SMELL. The 500-line allocation totals exactly 500, undercounts thirteen public types as eleven, and leaves no room for two seams or corrections. Raise the ceiling with justification or reduce the public scanner surface and use shared table-driven fixtures.

14. MINOR — Size criterion — SMELL. The outer review criterion still says 300 lines while task AC16 says 500. Update the stale review criterion so closure applies one threshold.

15. MINOR — Construction wording — SMELL. “Never constructs” conflicts literally with the required projection tests. Change it to “production never constructs” and expressly permit test construction.

16. MINOR — Source compatibility — SMELL. The handoff requirement mentions only #[must_use], but the return-type change can also break function-pointer, closure-return, and explicit-unit consumers. Document that complete compatibility boundary.

Verdict: not ready to plan; resolve blockers 1–9 first.
```

```
ROUND 1 FINDINGS (already folded once; kinds recurred)
BLOCKER

1. WRONG — Report vocabulary / field privacy. Public enum variant fields such as `Incomplete { skipped_entries }` and `Custody { state, assessment }` cannot remain private in Rust, so the specified API and AC2 cannot both be implemented. Suggested resolution: use private-field wrapper structs as variant payloads, or explicitly exempt enum variant fields from the privacy requirement.

2. WRONG — Checked scanner / signature and child-name identity. The signature and row type that increment 2 must preserve are unspecified. An implementation using `String` or `to_string_lossy()` would corrupt a non-UTF-8 `DirEntry::file_name()`, causing the later sibling guard to inspect a different name. Suggested resolution: freeze the checked result and row types now, retaining the enumerated name losslessly as `OsString`/`&OsStr`, and require that exact value for descriptor-relative reads and the later guard.

3. WRONG — Increment boundary / deferred public taxonomy. `IneligiblePopulationV1`, `CannotConstructSubjectV1`, and `CustodyStateSnapshotV1` are load-bearing public shapes, but their variants, payloads, and state mapping are left to the implementer. A generic reason enum could satisfy this increment yet fail to represent increment 2 without a public API change. Suggested resolution: specify increment 2’s exact population and guard cases plus custody-state mapping, or use extensible opaque payload structs with stable accessors.

4. WRONG — Ownership exclusion / AC6. “No ownership input, parameter, or variant anywhere” contradicts the existing public `decide_unused_candidate(..., recovery_owned: bool, ...)` surface. Literal compliance could make an implementer remove that parameter, breaking callers and behavior preservation. Suggested resolution: say this increment adds no new ownership input, variants, or plumbing and preserves the existing parameter unchanged.

5. WRONG — Canonicalization and compatibility behavior. The production exact-absence sweep currently enumerates the canonical root, while `scan_worktree_records` enumerates the caller-supplied spelling. Canonicalizing inside the compatibility wrapper changes paths/logs for symlink or relative roots; scanning the raw path in production changes exact-sweep behavior. Suggested resolution: define `requested_root`, `canonical_root`, enumeration path, and `record_path` construction separately for both entry points, with alias tests.

6. WRONG — Custody-root observation semantics. `Pinned`, `Unavailable`, and `IdentityChanged` are not operationally defined. Because enumeration and pinning currently use independent opens, replacing the root between them can enumerate names from object A and read records from object B while still reporting `Pinned`. Suggested resolution: specify before/pin/after identity checks, error-to-status mapping, precedence, and whether legacy enumeration continues after custody pin failure.

7. WRONG — Scan completeness and ordering. `skipped_entries` does not say whether it counts `ReadDir` item errors, invalid/unreadable legacy sidecars, or both; ordering is also unstated. Implementations can therefore publish different reports and log sequences for the same directory. Suggested resolution: define the counted population, continuation rules, selected-record population, treatment of unreadable custody records, and preservation of existing iterator order.

8. WRONG — Characterization evidence / AC4. No existing fixture directly exercises the sweep or decision helpers, so “every existing fixture” is vacuous. A readable `Preserved` record with a valid claim, vanished target, and `BothAbsent` currently reaches `Authorized`; an accidental refusal in this increment could pass a minimal test set despite violating behavior preservation. Suggested resolution: require a closed characterization matrix covering that known result, every legacy and custody guard refusal, missing/invalid claims, unreadable custody, all probe observations, and probe errors.

9. WRONG — Effect-freedom scope. The public `&dyn ExactAbsenceProbeV1` may be implemented downstream with arbitrary writes, so a transitive source audit cannot prove effect freedom for every invocation; a write-then-restore implementation also defeats byte snapshots. The spec additionally refers to two trait methods when there is only one. Suggested resolution: scope the proof to production wiring through `HostGitWorktree::observe_exact_absence`, audit that concrete call path, and correct the method count.

MAJOR

10. SMELL — Partial-enumeration evidence. A deterministic `ReadDir` item error is generally difficult to construct on ordinary local filesystems; making a child unreadable often still yields its name. Suggested resolution: require an injected enumeration seam or name a fault-capable filesystem and distinguish executed from unavailable evidence.

11. SMELL — Projection-totality claim. A table containing today’s variants does not ensure a future variant makes the test fail; production could update `decision()` without updating the table. Suggested resolution: require an exhaustive test-side match and describe compile-time exhaustiveness separately from behavioral evidence.

12. SMELL — 300-line cap. Eleven public types and accessors, scanner and sweep changes, several test matrices, and the required handoff make the cap high-risk and may encourage omission or compressed evidence. Suggested resolution: provide a line budget, raise or pre-waive the cap, or split the work further.

MINOR

13. WRONG — Source-compatibility statement. `#[must_use]` makes a discarded statement-position call warn and can break downstream builds using `-D warnings`. Suggested resolution: describe the return change as type-compatible but potentially lint-incompatible, and require explicit `let _ =` at warning-denying callers.

Disagreement resolved: Rigor is right that a 300-line breach is not proven inevitable, so sizing is MAJOR rather than BLOCKER; Soundness is right that the cap remains materially risky.

VERDICT: REJECT — before planning, make the public vocabulary and checked-scanner contract concrete, reconcile ownership and privacy requirements, define canonicalization/identity/completeness semantics, require a non-vacuous characterization matrix, and scope effect-freedom to production wiring.
```

### The spec you are correcting

```markdown
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
must not pretend to.

Precisely: **add no NEW ownership input, variant, or plumbing** — no
`LocallyOwned`, no `OwnershipCannotProve`, no new ownership parameter.
`decide_unused_candidate` already takes `recovery_owned: bool` and both production
call sites pass `false`. **Leave that parameter and those call sites exactly as
they are.** Removing it would break behavior preservation and is not what this
exclusion means.

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

**Privacy, stated in a form Rust can express.** A public enum's variant fields are
always public — `Incomplete { skipped_entries }` and `Custody { state, assessment }`
cannot have private fields. So: **structs** (`ExactAbsenceSweepReportV1`,
`ExactAbsenceScanStatusV1`, `ExactAbsenceSweepEntryV1`) keep private fields with
read-only accessors; **enum variant payloads** are either a single named type or
plain data, and privacy is not required of them. Where a payload needs to stay
evolvable, make it a private-field struct and have the variant carry that struct.
Do not contort the enums to satisfy a privacy rule that does not apply to them.

**Freeze the deferred taxonomy now — increment 2 must not need a public API
change.** These are load-bearing for increment 2, so define them here as closed
enums, exactly:

```rust
pub enum IneligiblePopulationV1 {
    /// `ProtectionPrepared` with no claim. NOT "missing claim" — its claim is
    /// schema-optional. Populated by increment 2.
    BareProtectionPrepared,
    /// Any state that is not a candidate population. Populated by increment 2.
    StateNotCandidate,
}

pub enum CannotConstructSubjectV1 {
    /// Guard 1. Populated by increment 2.
    RecordedWorktreePathNotAbsolute,
    /// Guard 2. Populated by increment 2.
    OutsideSweepRoot,
    /// Guard 3. Populated by increment 2.
    RecordFileNotExpectedSibling,
    /// Claim present but its bound authority could not be constructed.
    /// Increment 3 refines this into a typed object/reason product; keep it a
    /// single arm here. Populated by increment 2.
    ClaimAuthorityUnavailable,
}
```

`CustodyStateSnapshotV1` records the record's state kind and, where the state
carries one, its `PreservationReasonV1` — **without** holding the whole record.
Give it private fields and accessors.

Increment 2's admission table, which these must express, is:
`ProtectionPrepared` without a claim ⇒ `BareProtectionPrepared`;
`ProtectionPrepared` with a claim and `PreservationUnknown(MaterializationInFlight)`
with a claim ⇒ continue to construction; the other five `PreservationUnknown`
reasons, `PreservationPrepared`, `Preserved`, and every claim-forbidden state ⇒
`StateNotCandidate`. You are **not** implementing that table in this increment —
it is given so the types you freeze can express it.

**Do not add** an `InvalidStateClaimPair` arm. The canonical decoder already
rejects invalid required/forbidden claim pairs, so those records stay
`UnreadableCustody(Decode(..))`; a dormant arm for them would be unreachable by
construction.

### 2. The projection

`ExactAbsenceRecordAssessmentV1::decision() -> UnusedCandidateDecisionV1`,
**exhaustive, no wildcard**: `Legacy` and `Custody { assessment: Assessed(d) }`
return their contained decision; `UnreadableCustody`, `IneligiblePopulation` and
`CannotConstructSubject` project to `Refused`.

### 3. The checked scanner — freeze its types and its semantics

Add a checked scan whose result and row types are **frozen here**, because
increment 2's guards consume them and must not force a public API change:

```rust
pub struct CheckedScanV1 {
    canonical_root: Option<String>,
    status: ExactAbsenceScanStatusV1,
    rows: Vec<CheckedScanRowV1>,
}

pub struct CheckedScanRowV1 {
    record_path: String,
    /// The name exactly as `DirEntry::file_name()` produced it. **`OsString`, not
    /// `String`.** A `to_string_lossy()` here would corrupt a non-UTF-8 name and
    /// make increment 2's sibling guard compare a different name than the one on
    /// disk. Increment 2 must use this value, not a re-derived one.
    enumerated_name: std::ffi::OsString,
    scanned: ScannedWorktreeRecordV1,
}
```

Fields private with accessors. The accessor for `enumerated_name` returns
`&std::ffi::OsStr`.

**Enumeration path and root fields — define them per entry point, because the two
differ today.** `sweep_orphans_with_exact_absence` canonicalizes its argument and
enumerates the **canonical** root; other `scan_worktree_records` callers enumerate
the **caller-supplied** spelling. Preserve both behaviours exactly:

- `requested_root` = the string passed to `sweep_orphans_with_exact_absence`,
  verbatim.
- `canonical_root` = `Some(..)` when `canonicalize_lenient` succeeds, else `None`.
- The exact-absence sweep **enumerates the canonical root**, as today, and
  `record_path` is built from it exactly as today.
- `scan_worktree_records(root)` **keeps enumerating the raw argument**. Do **not**
  canonicalize inside the compatibility wrapper — that would change paths and log
  lines for symlinked or relative roots.
- Add a test with a symlinked root alias asserting both entry points still produce
  the paths they produce today.

**`ExactAbsenceEnumerationV1` semantics — define exactly what is counted:**

- `Refused(CannotCanonicalize)` when `canonicalize_lenient` fails. No enumeration
  is attempted; `entries` is empty.
- `Refused(CannotEnumerate)` when `read_dir` on the enumeration path fails. No
  entries.
- `Incomplete { skipped_entries }` when `read_dir` succeeded but one or more
  **iterator items** returned `Err`. `skipped_entries` counts **only** those
  per-item enumeration errors. Enumeration **continues** past them.
- `Complete` otherwise.

**Explicitly NOT counted as skipped:** a custody record that fails to decode, or a
legacy sidecar that fails to parse. Those are records that were successfully
enumerated and become `UnreadableCustody(..)` / their existing legacy outcome, and
must appear as entries. Say this in a doc comment — the distinction is exactly what
makes the count meaningful.

**Order is preserved.** Entries appear in the same order the current
implementation logs them, so log sequences do not change.

**`CustodyRootObservationV1` semantics — define them operationally.** Today
enumeration and any custody-root pin are independent opens, so a root replaced
between them could be enumerated as object A while records are read from object B.
Specify:

- `Pinned` — the custody root's identity was observed **before** enumeration and
  re-observed **after**, and both observations matched.
- `IdentityChanged` — both observations succeeded and differed.
- `Unavailable` — either observation failed for any reason.
- Precedence: this observation is independent of `ExactAbsenceEnumerationV1`;
  report both. If the identity cannot be observed, enumeration still proceeds and
  the entries stand — this increment changes no decision, and the field exists so a
  later increment can refuse on it.

If the current code performs no custody-root pin at all, say so in the handoff and
report `Unavailable`, rather than inventing a pin in this increment.

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

The return type is added to a function that previously returned `()`. That is
**type-compatible** for statement-position callers but **lint-incompatible**:
`#[must_use]` makes a discarded call warn, and this workspace builds under
`-D warnings`. Add an explicit `let _ = …;` at the `sweep_orphans` call site, and
note the lint consequence for any downstream caller in the handoff.

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

### Characterization is NOT "keep existing fixtures green"

**Measured: there is no existing test that exercises `sweep_orphans_with_exact_absence`
at all** — the only occurrences are its definition and its single production call.
So "existing fixtures keep passing" would be a vacuous claim. You must **write** the
characterization matrix, and it must pin today's behaviour including the parts that
are wrong.

Build a closed matrix over the production entry point with a programmable probe,
asserting the **projected decision** and the typed entry for each row:

| Row | Today's result — pin it |
|---|---|
| Readable `Preserved` record, valid complete claim, target vanished, probe says `BothAbsent` | **`Authorized`** |
| The same for `PreservationPrepared` and for `PreservationUnknown` with each reason | today's result |
| `ProtectionPrepared` with a claim, and without a claim | today's result |
| Claim-forbidden states | today's result |
| Record whose worktree is outside the sweep root | `Refused` |
| Record whose file is not the expected custody sibling | `Refused` |
| Unreadable / undecodable custody record | `Refused`, entry is `UnreadableCustody(..)` |
| Legacy sidecar: matching, non-matching, outside root | today's results |
| Probe returns `TargetPresent`, `RegisteredButAbsent`, `BothAbsent`, and `Err` | today's results |

**The first row is the important one.** A readable `Preserved` record with a
vanished target currently yields `Authorized` — that is the fail-open increment 2
closes. Pinning it as *currently `Authorized`* is what makes increment 2's flip to
`StateNotCandidate` a genuine behavioral red. If this increment accidentally
refuses it, behavior preservation is violated and a thin test set would not notice.

### Truthful scan status — new observable surface, test it directly

- root that cannot be canonicalized ⇒ `Refused(CannotCanonicalize)`;
- root that cannot be enumerated ⇒ `Refused(CannotEnumerate)`;
- clean enumeration ⇒ `Complete`;
- a decode-failing record and a bad legacy sidecar ⇒ **still `Complete`**, and both
  appear as entries — this pins the "not counted as skipped" rule;
- `Incomplete { skipped_entries }` requires a **per-item `ReadDir` error**, which is
  hard to construct deterministically on ordinary local filesystems (making a child
  unreadable usually still yields its name). Use an **injected enumeration seam** so
  the count is testable deterministically. If you instead rely on a real filesystem
  fault, name the environment that can produce it and mark the test not-executed
  where it cannot run.
- `CustodyRootObservationV1`: test each value you can construct; mark the others
  not-executed and say why.

### Projection totality

Assert `decision()` arm-by-arm in a table-driven test **using an exhaustive
`match` on the test side**, so adding a production variant fails to compile in the
test rather than silently passing a stale table. Describe compile-time
exhaustiveness (the `match` with no wildcard) and behavioral coverage (the table)
as **separate** claims in the handoff — the first is a compiler guarantee, the
second is evidence.

### Effect-freedom evidence

Byte snapshots are **not** sufficient alone — they prove final-state equality and
cannot exclude a helper that mutates and restores.

Scope the proof to **production wiring**, not to the trait: `&dyn
ExactAbsenceProbeV1` is public and a downstream implementation could do anything,
so no source audit can cover every possible invocation. Audit the concrete
production path: `sweep_orphans_with_exact_absence` → the checked scanner → guards
and existing decision helpers → `HostGitWorktree::observe_exact_absence` (the
trait's **single** method).

Allowed leaves: bounded reads and decoding, descriptor and metadata observation,
canonicalization and identity checks, `git rev-parse`, `git worktree list
--porcelain -z`, allocation, collection, tracing.

The audit must show **no edge** to provider removal or pruning, `remove_dir_all`,
unlink or rename, custody publication or replacement, settlement, transitions, or
backend cleanup. Record it in the handoff as a call-path list, and state explicitly
that it covers the production wiring only. Byte snapshots stay as corroborating
regressions.

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
   `sweep_orphans` discards it via an explicit `let _ =` and its existing
   independent second scan is unchanged.
2. The report vocabulary exists. **Structs** carry private fields with read-only
   accessors; enum variant payloads are exempt from that requirement, as Rust
   requires. `decision()` is exhaustive with no wildcard.
3. `IneligiblePopulationV1`, `CannotConstructSubjectV1` and `CustodyStateSnapshotV1`
   are frozen exactly as specified, so increment 2 populates them without a public
   API change. No `InvalidStateClaimPair` arm.
4. `CheckedScanV1` / `CheckedScanRowV1` are frozen, and `enumerated_name` is an
   `OsString` carrying `DirEntry::file_name()` losslessly — no `to_string_lossy()`.
5. **Enumeration paths are unchanged per entry point**: the exact-absence sweep
   enumerates the canonical root; `scan_worktree_records` keeps enumerating its raw
   argument. A symlinked-root alias test asserts both still produce today's paths.
6. `ExactAbsenceEnumerationV1` follows the stated semantics: `skipped_entries`
   counts **only** per-item `ReadDir` errors; decode failures and bad legacy
   sidecars are **not** skipped and do appear as entries; enumeration continues past
   skipped items; entry order is unchanged.
7. `CustodyRootObservationV1` follows the stated before/after semantics, or the
   handoff states that no pin exists today and reports `Unavailable`.
8. **No decision changes.** The characterization matrix above exists and pins
   today's projected decision for every row — **including the `Preserved` + valid
   claim + vanished target + `BothAbsent` ⇒ `Authorized` row**, which increment 2
   will flip.
9. Scan-status tests cover the listed cases; `Incomplete` uses an injected
   enumeration seam or names the environment that can produce a real per-item error,
   with each test honestly marked executed or not-executed.
10. `decision()` totality is asserted with an exhaustive test-side `match`, and the
    handoff separates the compile-time guarantee from the behavioral coverage.
11. The effect-freedom audit is recorded as a call-path list **scoped to production
    wiring** through `HostGitWorktree::observe_exact_absence`, and the handoff says
    so rather than claiming the trait is effect-free in general.
12. The unconstructed arms exist, are documented as increment-2 wiring, and are not
    removed, `cfg(test)`-gated, or fake-constructed.
13. **No NEW ownership input, variant, or plumbing**, and `decide_unused_candidate`
    keeps its existing `recovery_owned` parameter and its two `false` call sites
    unchanged.
14. No custody state, transition, publication, settlement, deletion, or CLI
    call-site behavior changes; no new `bridge-core` surface; no async proof trait;
    no change to `compare_path_identities` or `host_git.rs`'s proof.
15. The handoff is created at
    `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-increment1-handoff.md`, with
    the marked operator-evidence section and its pending placeholders, and states
    plainly that this increment has no genuine behavioral-red test and why.
16. `git diff --numstat 9aedf175..HEAD` at most **500** changed lines including
    tests and the handoff, measured on a **clean, fully committed worktree** — the
    command ignores staged, unstaged and untracked bytes, so an uncommitted handoff
    would let a breach read green. The cap was raised from 300 after the spec review
    observed that eleven public types with accessors, the scanner change, the
    characterization matrix, the status tests and the handoff will not fit;
    indicative budget: ~180 types and accessors, ~60 scanner and sweep wiring, ~200
    tests, ~60 handoff. A breach still requires an explicit pre-closure operator
    waiver. If you project a breach, say so before implementing rather than after.
17. Report test totals as the count of test binaries plus doc-test suites, not by
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
```

## Acceptance Criteria

### Required output format

Emit **only** the corrected spec file, beginning with exactly:

```
---
task-type: implement
---
```

then a `#` title, then these sections with these exact headings, in this order:
`## Description`, `## Acceptance Criteria`, `## Files`, `## Spec Refs`,
`## Commit Message`. Description and Acceptance Criteria are required by the schema.
Put **only** the commit message under `## Commit Message` — any instruction prose
there becomes the commit subject.

Include a falsification license telling the implementer that every anchor and
behavioural claim is an operator claim it may disprove against the repository, and
that finding the work smaller than described is a good outcome. Prefer symbol names
over line-number anchors.

### The spec you produce must

1. **Resolve all sixteen round-2 findings**, with the literal artefacts they ask
   for: the checked-scanner signature and canonicalization ownership; the complete
   `CustodyStateSnapshotV1` fields, types, accessors and conversion mapping; a
   `ClaimAuthorityUnavailable` shape that increment 3 can extend without a breaking
   change; the report/entry identity handling for non-UTF-8 names; the
   `CustodyRootObservationV1` semantics that actually bind the enumerated
   directory rather than a re-walked path; and the effect-freedom allowed-leaf list
   corrected to cover `read_dir` traversal and legacy `std::fs::read`.
2. **Give the characterization matrix concrete expected values**, not "today's
   result". Read the code and state what each row yields on `9aedf175`. The row that
   matters most: a readable `Preserved` record with a valid complete claim, a
   vanished target, and a probe answering `BothAbsent`. Pinning its current value is
   what makes increment 2's change a genuine behavioral red.
3. **Preserve behaviour exactly**, including today's silent omission of malformed
   legacy sidecars and the different enumeration roots used by the exact-absence
   sweep versus `scan_worktree_records`.
4. **Be sized honestly.** Set a numstat cap you believe after counting the public
   types, accessors, scanner wiring, matrices and handoff, and give an indicative
   per-area budget. Require it measured on a clean committed worktree, since the
   command ignores staged and untracked bytes. If the work does not fit one
   increment, say so and propose the split rather than setting a cap that forces
   omission.
5. **Assign gate execution.** The implement container has no compile loop, so name
   the operator as the executor of the final host gates and require a marked
   operator-evidence section in the handoff with pending placeholders. Name the
   handoff path.
6. **State the evidence honestly**: what is genuinely red, what is characterization,
   what is a compiler guarantee rather than a test, and which checks are
   environment-sensitive. This lane has shipped three tests that looked like
   evidence and were not; one passed on macOS/APFS and on the container's overlayfs
   and failed only on ubuntu/ext4.
7. **Not require anything unimplementable.** Round 1 caught a demand that public
   enum variant fields be private. Re-check every requirement against what Rust and
   this codebase actually permit.

## Spec Refs

Authoritative and present in this checkout: `crates/bridge-worktree/src/sweep.rs`,
`crates/bridge-worktree/src/custody.rs`, `crates/bridge-worktree/src/host_git.rs`,
`bin/a2a-bridge/src/main.rs`.

The three-increment design lives on a planning branch and is **not** in this
checkout; its load-bearing conclusions are reproduced above, and its absence is not
a missing input.
