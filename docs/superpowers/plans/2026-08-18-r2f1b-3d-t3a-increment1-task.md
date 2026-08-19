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
