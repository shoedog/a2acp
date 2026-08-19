---
task-type: design
---

# Author revision 2 of the T3a increment 1 slice A task spec

## Description

Produce the **complete revised implementation task spec** for T3a increment 1 slice
A, with every round-2 finding folded and the whole document internally consistent.
Emit it between the extraction markers. The session cwd is checked out at `main` =
`9aedf175` and is authoritative for every factual claim.

### Why you are authoring rather than reviewing

This spec's history, plainly:

- Two earlier specs for this work each failed two review rounds (7→8 blockers, then
  9→9) and were abandoned.
- This spec was assembled by transcribing **your own** design content into task-spec
  form. Round 1 returned 5 blockers / 12 findings — the first convergence this work
  has shown, and both lenses confirmed the characterization matrix is correct.
- The operator then hand-folded those 12 findings across a 600-line document. Round 2
  returned **7** blockers, and **four of them are contradictions the operator
  introduced while folding** — `effective_decision_at` still referenced three times
  after being forbidden, a crate-private claim left standing beside a now-public
  type, a superseded seam-test bullet, and two different line caps.

The hand-folding step is the defect. So you fold, and you own the whole document
afterwards. Removing every trace of what a folded finding supersedes is the single
most important thing you do here.

### The one artefact nobody has written yet

Round-2 blocker 1 is correct and is the crux: the spec **demands** literal
declarations for fifteen public types while supplying only names. Your earlier design
listed names too. **Write the declarations.** Real Rust, every variant, payload,
field, derive, accessor signature and conversion. Do not emit an instruction telling
an implementer to write them — that instruction is what has failed twice.

### Settled context — do not re-open

- **T3a decides; T3b acts.** No write, rename or unlink on any path in this slice.
- **Slice A of two.** Slice A: public shape, raw/effective projections,
  compatibility-backed report, injectable seam, characterization — root authority
  truthfully `Unavailable` throughout. Slice B: descriptor enumeration, pinned-root
  classification, platform gating, real-Git authority evidence. You proposed this
  split and the owner chose it.
- **Behavior-preserving.** Today's flow is eager: `scan_worktree_records` returns a
  `Vec`, so everything is enumerated and read before anything is assessed or logged.
  Preserve that two-phase ordering. `PinnedDirectoryV1::open(..).ok()` means a pin
  failure leaves legacy rows proceeding while custody rows become the "not pinnable"
  refusal — preserve that too.
- **No NEW ownership** input, variant or plumbing; `decide_unused_candidate` keeps its
  `recovery_owned` parameter and both `false` call sites untouched.
- The **policy-readiness gate** stays: `false` until increment 2's admission rule
  lands, so the interval after slice B can produce `Pinned` cannot make a `Preserved`
  record with a vanished target effectively `Authorized`.
- The **characterization matrix is correct** at `9aedf175` — both review lenses
  agreed, including the load-bearing `Preserved` + valid claim + vanished target +
  `BothAbsent` ⇒ raw `Authorized` row and the silently-omitted malformed legacy
  sidecar row. Keep it. Do not weaken it.
- Slice A has **no behavioral red**; its only base-compatible runtime red is an
  API-shape assertion. Say so honestly; do not manufacture one.
- The container running the implementation has **no compile loop**, so the operator
  runs the final host gates. Keep the `## OPERATOR EVIDENCE — PENDING` block.

### Round-2 findings to fold

Two are genuinely new and must be resolved, not merely restated: `effective()`
returns a `Copy` tuple so a consumer can still separate a decision from its entry —
the binding is advisory, not enforced; and a generic fake scanner can stay green
while the real compatibility source diverges, so pin-failure evidence needs the real
source.

Where a finding asks for something impossible, or wrong about the code, say so inside
the document with the evidence rather than folding it silently.

```
ROUND 2 FINDINGS
PRIOR-ROUND ADJUDICATION

- FIXED — Public type count: corrected from fourteen to fifteen.
- FIXED — Policy-readiness gate: added and held false until increment 2.
- PARTIAL — Two-phase traversal: production ordering is corrected, but the seam-test wording still permits assessment/logging during enumeration.
- PARTIAL — Pin failure: required compatibility behavior is corrected, but no deterministic mechanism proves the compatibility implementation preserves it.
- PARTIAL — Observation visibility: the public/raw distinction is explained correctly, but contradicted later by “all seam and observation types stay crate-private.”
- The other prior-round findings are not individually identified in the supplied reviews, so they cannot be separately adjudicated.

1. BLOCKER / WRONG — Public vocabulary / Acceptance 1. Issue: The spec requires a frozen, literal fifteen-type API while supplying only names and selected fragments. Two compliant implementers could choose incompatible variants, payloads, fields, accessors, or conversions, forcing slice B or increment 2 into a breaking redesign. Suggested resolution: Include the complete Rust declarations in the spec or incorporate a separately reviewed API appendix verbatim.

2. BLOCKER / WRONG — Raw and effective projection. Issue: The spec forbids `effective_decision_at` and requires `effective()`, but later directs future action code to call `effective_decision_at`; following either instruction violates another acceptance requirement. Suggested resolution: Replace every stale reference, including provenance and action guidance, with `effective()` or “the effective entry/decision pair.”

3. BLOCKER / WRONG — Scanner visibility. Issue: `CustodyRootObservationV1` is required to be public, while the scanner section says all observation types are crate-private; an implementer could hide a required public type. Suggested resolution: Say “all seam and raw observation types stay crate-private” consistently.

4. BLOCKER / WRONG — Bound-pair safety claim. Issue: `effective()` returns a tuple containing a by-value, `Copy` decision, so consumers can collect decisions separately, reorder entries, and apply one row’s authorization to another. The claimed invariant that decisions “can never be separated” is therefore false. Suggested resolution: Return an opaque effective-entry/action-capability view that retains row identity, or explicitly downgrade row binding to a convention and specify the later action-side identity check.

5. BLOCKER / WRONG — Scan flow / seam tests. Issue: “Each row processed before the next `next_name` call” can require assessment and logging during enumeration, contradicting the mandatory eager two-phase flow and allowing Git probes to observe state before enumeration is drained. Suggested resolution: Require two explicit assertions: each yielded record is read and collected before the next name, and no assessment, probe, or decision event occurs until `next_name` returns `None` and `finish` completes.

6. BLOCKER / WRONG — Compatibility pin-failure evidence. Issue: The generic fake scanner can remain green even if the real compatibility source converts pin failure into an open refusal, which would omit legacy and custody rows and their logs. Suggested resolution: Inject a deterministic pin opener into the compatibility source, or provide another discriminating test mechanism that exercises successful `read_dir` followed by pin failure in that implementation.

7. BLOCKER / WRONG — Acceptance 17 / sizing. Issue: The outer criterion requires 600 changed lines, the task says 700, and the component estimates total about 760. The mandatory pre-edit rule therefore requires the implementer to stop before editing. Suggested resolution: Re-slice the work or establish one authoritative cap and an explicit waiver supported by a credible component estimate beneath it.

8. MAJOR / SMELL — Scanner seam / classifier declarations. Issue: `CheckedScanOpenRefusalV1`, `CheckedScanEntryRefusalV1`, `RootObservationSetV1`, and the identity-capture representation are referenced but not defined. In particular, an incautious reuse of `DirectoryIdentityV1::matches` could treat unavailable birthtime differently from the required `Unavailable` result. Suggested resolution: Specify the private declarations and an explicit comparison over complete `(dev, ino, birthtime)` captures.

9. MAJOR / SMELL — Public safety boundary. Issue: Public `is_authoritative()` can be combined with raw `decision()`, allowing future code to bypass policy readiness even after the tuple-binding problem is fixed. Suggested resolution: Separate raw reporting from action eligibility and rename `is_authoritative()` to express scan authority rather than action authority.

10. MAJOR / SMELL — Slice boundary. Issue: Slice A includes the complete three-capture classifier even though production supplies an empty observation set and pinned-root classification is assigned to slice B. Suggested resolution: Defer the private classifier and its non-production tests to slice B unless slice B demonstrably needs its exact internal shape landed first.

11. MAJOR / SMELL — Decomposition. Issue: Adding fifteen public types, policy projections, scanner abstractions, traversal, compatibility behavior, and extensive tests to the existing large `sweep.rs` increases coupling and review burden. Suggested resolution: Consider a report/vocabulary module plus a private checked-scanner module without expanding `bridge-core` surface.

12. MINOR / SMELL — Slice-B exclusions. Issue: “No root pinning” is imprecise because compatibility behavior retains the existing `PinnedDirectoryV1` custody-read pin. Suggested resolution: Say slice A adds no authoritative enumeration-root pinning beyond the existing custody-read pin.

Disagreement resolution: The Rigor lens is right to gate the tuple-binding and two-phase-test defects because each admits a concrete wrong authorization or behavior change; the Soundness lens is right that classifier placement and module decomposition remain non-gating design advice.

VERDICT: NOT READY TO PLAN — first freeze the complete API, remove the contradictory instructions, make row binding and compatibility pin-failure evidence enforceable, clarify two-phase testing, and reconcile the line cap with the estimated scope.
```

```
ROUND 1 FINDINGS — already folded once; check the fold actually took
BLOCKER

1. WRONG — Section: Scan flow step 4 / behavior preservation.
Issue: Processing each row before requesting the next name reverses today’s enumerate-and-read-all, then probe-and-log ordering. If probing row A creates, removes, or rewrites row B, the specified implementation observes a different result.
Suggested resolution: Preserve two phases—enumerate/read all intermediate rows, finish enumeration, then assess and log—or explicitly declare the concurrency-semantics change and provide behavioral evidence.

2. WRONG — Section: Public vocabulary / Acceptance 1.
Issue: The spec names fifteen public types while requiring fourteen, leaves several variants, payloads, fields, derives, conversions, and accessors undefined, and simultaneously makes CustodyRootObservationV1 public while declaring all observation types crate-private. Implementers can produce incompatible APIs, and slice B may require a breaking change.
Suggested resolution: Supply literal Rust declarations or exhaustive equivalent tables for every public type; correct the count; define all assessment mappings and construction signatures; and explicitly distinguish public CustodyRootObservationV1 from private raw observation types.

3. WRONG — Section: Scanner seam / classifier contract.
Issue: RootObservationSetV1 and “every required observation” are undefined. An implementation using only two snapshots could report Pinned even though enumeration, custody reads, and final named-root observation refer to different filesystem objects.
Suggested resolution: Define the retained enumerator identity, pinned custody-directory identity, final no-follow named-root observation, capture times, identity-comparison rules, usability rules, and complete precedence for Pinned, IdentityChanged, and Unavailable.

4. WRONG — Section: Compatibility scanner open behavior.
Issue: Pin-creation failure is not assigned a result. With successful read_dir but failed PinnedDirectoryV1 creation, returning CannotEnumerate would omit rows and logs that current behavior preserves: legacy rows still proceed, while custody rows become UnreadableCustody.
Suggested resolution: Specify that open refuses only when read_dir fails, retains any pin failure in session state, and reproduces today’s per-entry legacy and custody outcomes.

5. WRONG — Section: Public report / Acceptance 2.
Issue: is_authoritative() lacks a truth table. For an incomplete enumeration with stable root identity, “Pinned alone” returns true while “Complete and Pinned” returns false, producing incompatible authority decisions.
Suggested resolution: Define it explicitly—presumably enumeration is Complete and custody-root observation is Pinned—and add acceptance cases for every scan/root combination.

MAJOR

6. SMELL — Section: Effective projection API.
Issue: effective_decision_at(index) separates a decision from its governing entry; filtering or reordering entries can apply row A’s Authorized result to row B.
Suggested resolution: Prefer an accessor or iterator returning the entry and effective decision as a bound pair, and distinguish raw observations from action-capable decisions through type or visibility.

7. SMELL — Section: Split sequencing.
Issue: After slice B can produce Pinned, Complete + Pinned + Preserved + BothAbsent becomes effectively Authorized before increment 2 installs its refusing admission rule.
Suggested resolution: Keep an internal policy-readiness gate false until admission lands, or explicitly prohibit consumers of effective authorization during the stacked interval.

8. SMELL — Section: Acceptance 17 sizing.
Issue: The 600-line cap has little credible margin for fifteen public types, projections, scanner implementations, classifier, traversal refactor, characterization tests, seam tests, and the approximately 97-line handoff.
Suggested resolution: Add a per-component line budget or split characterization/closure before dispatch; retain the mandatory pre-edit stop rather than compressing evidence.

9. SMELL — Section: Scan status and characterization precision.
Issue: “Canonicalizes once” does not name canonicalize_lenient, so a missing root could be tested as CannotCanonicalize instead of today’s CannotEnumerate. The positive legacy row also omits the condition that source/common authority must construct a candidate.
Suggested resolution: Name the exact helper and qualify the legacy probe-mapping row with successful candidate construction.

MINOR

10. SMELL — Section: Platform instructions.
Issue: “No platform gating” conflicts in wording with required non-Unix dead-code allowances and Unix-only fixtures.
Suggested resolution: Clarify that production functionality must not be excluded, while lint-only cfg_attr annotations and inherently Unix-only tests remain permitted.

11. SMELL — Section: Seam evidence.
Issue: “Zero log-helper calls” names no existing helper or injected interface; production currently logs through tracing.
Suggested resolution: Define the observable as zero matching tracing events or introduce a private reporter seam used by production.

12. SMELL — Section: Handoff acceptance.
Issue: The required “marked operator-evidence section” has no literal heading or placeholder syntax.
Suggested resolution: Provide the exact marker and placeholders the implementer must preserve.

Lens resolution: There is no substantive conflict; Soundness’s explicit three-authority observation model is adopted as the stronger resolution of Rigor’s undefined-observation blocker, and both lenses agree the characterization matrix—including raw Authorized for the load-bearing Preserved/BothAbsent row and silent malformed-legacy omission—is correct at 9aedf175.

VERDICT: Not ready to plan; first fix the five BLOCKER items above.
```

### The document to revise

```markdown
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
- No new `bridge-core` surface. No `libc`, no `fdopendir`, and no
  **platform-conditional production functionality** in slice A — all of that is
  slice B. This does **not** forbid `#[cfg_attr(not(unix), allow(dead_code))]` lint
  annotations, nor `#[cfg(unix)]` on tests that are inherently Unix-only; both
  remain required where they apply.

## What to build

All in `crates/bridge-worktree/src/sweep.rs`.

### 1. Public vocabulary

`ExactAbsenceSweepReportV1` with **private** fields:

- `requested_root: String`
- `canonical_root: Option<String>`
- `scan: ExactAbsenceScanStatusV1`
- `entries: Vec<ExactAbsenceSweepEntryV1>`

Expose read-only accessors, `is_authoritative()`, and a **bound-pair** effective
accessor:

```rust
pub fn is_authoritative(&self) -> bool;

/// Yields each entry together with its effective decision, so a decision can never
/// be separated from the entry that governs it.
pub fn effective(
    &self,
) -> impl Iterator<Item = (&ExactAbsenceSweepEntryV1, UnusedCandidateDecisionV1)>;
```

**Do not expose an index-keyed `effective_decision_at(index)`.** Filtering or
reordering entries would let one row's `Authorized` be applied to another. The pair
must travel together.

`is_authoritative()` truth table — define it exactly, and cover every combination in
tests:

| enumeration | custody root | `is_authoritative()` |
|---|---|---|
| `Complete` | `Pinned` | `true` |
| `Complete` | `IdentityChanged` or `Unavailable` | `false` |
| `Incomplete` or `Refused` | any | `false` |

In slice A it is always `false`, because root authority is always `Unavailable`.

**There are fifteen public types**: `ExactAbsenceSweepReportV1` plus the fourteen
below. All must exist after slice A.

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

**Public vs crate-private observation types — these are different things.**
`CustodyRootObservationV1` is **public**: it is the classified, three-valued result
(`Pinned` / `IdentityChanged` / `Unavailable`) that appears in the report. The **raw**
observation types the classifier consumes — `RootObservationSetV1` and the identity
captures inside it — are **crate-private**. Earlier wording said "all observation
types stay crate-private"; that was wrong and is corrected here.

**Supply literal Rust declarations for all fifteen public types**, including every
variant, payload, field, derive, accessor signature, and each `From`/conversion. Do
not leave any of them to implementer choice: slice B and increment 2 both build on
this surface, and an incompatible shape here forces a breaking change later. Derive
at minimum `Debug`, `Clone`, `PartialEq`, `Eq` on the value types so tests can assert
them directly.

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

The **effective** decision paired with each entry by `effective()` is stricter:

```text
enumeration not Complete                    -> Refused
custody root not Pinned                     -> Refused
policy not ready (see below)                -> Refused
Complete + Pinned + ready + Legacy          -> Refused
Complete + Pinned + ready + non-Legacy      -> the raw decision
```

Rationale to carry in doc comments: an incomplete scan may have skipped a
conflicting sibling record, so it is not future action authority; `Pinned` binds
enumeration and descriptor-relative custody reads but **not** legacy bytes reopened
through `read_sidecar`; and raw legacy decisions plus log output stay unchanged.

**Policy-readiness gate — required, and this is a cross-slice safety property.**
Add a crate-private readiness flag that the effective projection requires and that
**is `false` in slice A and stays `false` until increment 2's refusing admission
rule lands**. Without it, the moment slice B can produce `Pinned`, a
`Complete + Pinned` scan over a `Preserved` record with a vanished target and
`BothAbsent` would become *effectively* `Authorized` — the exact fail-open increment
2 exists to close — during the interval between the slices. Document the flag with
that reason, and add a test asserting the effective decision is `Refused` while it
is `false` even when the scan is otherwise authoritative.

**In slice A, root authority is always `Unavailable` and the readiness flag is
`false`,** so the effective decision is `Refused` for every row. That is correct and must be stated in the
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

**`open()` refuses ONLY when `read_dir` fails.** Today the code does
`PinnedDirectoryV1::open(..).ok()` and carries an `Option`: when the pin fails,
legacy rows still proceed normally and each custody row becomes
`UnreadableCustody(Unreadable("sweep root is not pinnable"))`. So a pin failure must
be **retained in session state**, not turned into `CannotEnumerate` — refusing the
whole open would omit rows and log lines that exist today. `read_custody` returns
that same refusal per entry when the pin is absent, and `read_legacy` is unaffected.
Cover this with a test: `read_dir` succeeds, pin fails, legacy rows present, custody
rows unreadable, enumeration `Complete`.

Provide an **injectable** test source so enumeration outcomes are deterministic. A
per-item `ReadDir` error is not reliably constructible on ordinary local
filesystems, so `Incomplete { skipped_entries }` must be tested through injection,
not by trying to provoke a real fault.

Include the pure classifier now with slice B's full contract, even though slice A
can only produce `Unavailable`. Define `RootObservationSetV1` (crate-private) as
**three** optional identity captures, each recorded with its capture point:

1. the **retained enumeration object** — the directory actually enumerated;
2. the **pinned custody directory** — the object descriptor-relative custody reads
   went through;
3. the **final no-follow open of the named root**, taken after enumeration finishes.

Identity comparison uses the repository's required `(dev, ino, birthtime)` tuple.
Classification, with complete precedence:

| condition | result |
|---|---|
| all three captures present **and** all three identities equal | `Pinned` |
| all three present and any pair differs | `IdentityChanged` |
| any capture absent, or any identity unusable (e.g. birthtime unavailable) | `Unavailable` |

`Unavailable` takes precedence over `IdentityChanged`: a mismatch cannot be asserted
from an incomplete set. Two captures are **not** sufficient for `Pinned` — with only
enumeration and final observations, custody reads could have gone through a
different object. State in the doc comment that this is the project's fail-closed
identity model, not a claim of immunity to filesystem object-ID reuse.

In slice A the set is empty, so the classifier yields `Unavailable`; slice B
populates it.

### 4. Scan flow, slice A

`sweep_orphans_with_exact_absence`:

1. Retains `requested_root` verbatim.
2. Canonicalizes once **with `canonicalize_lenient`, the helper the current code
   uses** — not `std::fs::canonicalize`. Only a `canonicalize_lenient` failure is
   `Refused(CannotCanonicalize)`; a root that is merely missing must still reach
   enumeration and yield today's `CannotEnumerate`. On `CannotCanonicalize`: root
   `Unavailable`, no entries.
3. Opens the compatibility source on the **canonical** root. If enumeration cannot
   start ⇒ `Refused(CannotEnumerate)`, no entries.
4. **Two phases, preserving today's ordering exactly.** Today
   `scan_worktree_records` returns a `Vec` — it enumerates and reads *everything*
   first, and only then does the caller probe and log. **Phase 1:** drain the
   session, reading each legacy or custody record, and collect the intermediate rows.
   **Phase 2:** after enumeration has finished, assess and log each row in order.
   Do **not** interleave probing with enumeration: probing runs `git worktree list`
   and target probes, and a streaming order would let those observe filesystem state
   that today's eager enumeration has already captured. That would be a behavior
   change in a behavior-preserving slice.
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
- malformed legacy omission with zero probe calls and **no emitted decision
  event** — production logs through `tracing`, so make the observable concrete:
  either assert zero matching `tracing` events with a capturing subscriber, or route
  production's per-row emission through a crate-private reporter seam the test can
  count. Say which you chose;
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
- The handoff carries a **marked operator-evidence section**, using exactly this
  heading and placeholder form so the operator can find and fill it mechanically:

  ```markdown
  ## OPERATOR EVIDENCE — PENDING
  - [ ] `cargo fmt --all -- --check` — PENDING OPERATOR
  - [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` — PENDING OPERATOR
  - [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — PENDING OPERATOR
  ```

  Leave the checkboxes unticked and the `PENDING OPERATOR` markers in place.
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

1. All **fifteen** public types exist, each given as a literal Rust declaration
   with its variants, payloads, fields, derives, accessors and conversions — none
   left to implementer choice. Public structs have private fields with read-only
   accessors; public enum variant payloads are ordinary and public.
1b. `CustodyRootObservationV1` is public; `RootObservationSetV1` and its raw identity
   captures are crate-private.
2. `ExactAbsenceSweepReportV1` exposes accessors, `is_authoritative()` implementing
   the stated truth table, and a **bound-pair** `effective()` iterator. There is **no**
   index-keyed `effective_decision_at`. Every scan/root combination in the truth
   table has a test.
3. Entries retain `record_path: String` **and** `enumerated_name: OsString`, with
   the accessor returning `&OsStr`. No `to_string_lossy()` on the enumerated name.
4. `CustodyStateSnapshotV1` is `{ kind, preservation_reason }`, its conversion
   exhaustively matches all ten states, only `PreservationUnknown` carries a reason,
   and `RecoveredLive` retains no digest.
5. `ClaimAuthorityObjectV1` and `ClaimAuthorityUnavailableReasonV1` are
   `#[non_exhaustive]`; `ClaimAuthorityUnavailableV1` has private fields with
   accessors.
6. `decision()` is exhaustive with no wildcard; the effective projection implements
   the stated table including the **policy-readiness gate**, which is `false` in
   slice A and documented as staying `false` until increment 2's admission rule
   lands. A test asserts the effective decision is `Refused` while the gate is
   `false` even for an otherwise authoritative scan. Doc comments record that slice A
   always yields `Refused`, and that action code must use the effective pair rather
   than the raw decision.
7. The crate-private seam traits exist with a compatibility implementation and an
   injectable test implementation. `open()` refuses **only** on `read_dir` failure;
   a pin failure is retained in session state and reproduces today's per-entry
   outcomes (legacy rows proceed, custody rows become the "not pinnable" refusal,
   enumeration `Complete`), with a test. The pure classifier implements the
   three-capture contract and precedence, `Unavailable` outranking
   `IdentityChanged`.
8. **No decision changes.** The characterization matrix above exists with these
   expected values, including the `Preserved` + valid claim + vanished target +
   `BothAbsent` ⇒ **raw `Authorized`** row and the silently-omitted malformed legacy
   sidecar row.
9. `scan_worktree_records` keeps every listed observable semantic, including
   enumerating the raw spelling; a symlinked-root alias test asserts both entry
   points still produce today's paths.
10. `skipped_entries` counts only iterator-item errors; a decode failure is emitted
    as an entry and does not increment it. **Enumeration and assessment stay in two
    phases** — all rows are enumerated and read before any is assessed or logged,
    matching today's eager `Vec` ordering. Canonicalization uses
    `canonicalize_lenient`, and a merely-missing root still yields `CannotEnumerate`,
    not `CannotCanonicalize`.
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
15. No `libc`, `fdopendir`, descriptor enumeration, root pinning, or
    platform-conditional **production functionality** — all slice B. Lint-only
    `cfg_attr` annotations and inherently Unix-only tests remain permitted. No new
    `bridge-core` surface. No custody state, transition, publication, settlement,
    deletion, or CLI behavior change.
16. The handoff is created from the installed template at
    `~/.claude/handoff-template.md` (resolve it; do not recreate it from memory) at
    `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-inc1-sliceA-handoff.md`, carries
    the `## OPERATOR EVIDENCE — PENDING` block verbatim with its markers intact, and
    states plainly that slice A has no behavioral red and why.
17. Target **at most 700 changed lines** including tests and handoff, measured by
    the operator on a clean committed tree. Indicative per-component budget:
    ~230 the fifteen public types with derives and accessors; ~60 projections and the
    readiness gate; ~90 the seam traits, compatibility source and classifier; ~40
    traversal and `sweep_orphans` wiring; ~180 characterization; ~60 seam tests; ~100
    handoff (the installed template is roughly that size before content). **Do a
    pre-edit estimate and stop before implementing if it exceeds the cap**, proposing
    the split — never compress evidence, binding, or handoff work to fit. A breach
    requires an explicit operator waiver.
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
```

## Acceptance Criteria

The document you emit must:

1. Begin with exactly `---`, `task-type: implement`, `---`, then a `#` title, then
   `## Description`, `## Acceptance Criteria`, `## Files`, `## Spec Refs`,
   `## Commit Message` — those headings, that order. Only the commit message goes
   under the last heading; instruction prose there becomes the commit subject.
2. Contain **literal Rust declarations for all fifteen public types**, plus the
   crate-private seam traits, refusal types, `RootObservationSetV1` and the
   classifier signature.
3. Be **internally consistent**: no reference to `effective_decision_at`, no
   surviving claim that all observation types are crate-private, no seam-test bullet
   contradicting the two-phase ordering, exactly one line cap stated once and
   consistent with any per-component budget.
4. Resolve the `Copy`-tuple separation concern with an actual mechanism, not a
   warning — or state plainly why no enforceable mechanism is worth it here.
5. Require pin-failure evidence through the **real** compatibility source, not only
   an injected fake.
6. Carry a falsification license telling the implementer every anchor is an operator
   claim it may disprove against the repository, and that finding the work smaller is
   a good outcome.
7. State a line cap you believe after counting what you actually wrote, with a
   per-component budget, measured on a clean committed tree, and a mandatory pre-edit
   stop if the estimate exceeds it. If slice A no longer fits one dispatch, say so in
   the document and propose the split rather than setting a cap that forces omission.
8. Prefer symbol names to line-number anchors.

## Spec Refs

Authoritative and present in this checkout: `crates/bridge-worktree/src/sweep.rs`,
`crates/bridge-worktree/src/custody.rs`, `crates/bridge-worktree/src/provider_path.rs`,
`crates/bridge-worktree/src/host_git.rs`, `bin/a2a-bridge/src/main.rs`.
