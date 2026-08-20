---
task-type: implement
---

# Closure fold — A2a task spec v4, seven findings, no restructuring

## Description

Your A2a spec v3 passed a verification review with seven findings: one gating,
six explicitly non-gating. The reviewer adjudicated all prior findings FIXED and
re-reported none. The round-3 seam question is closed — what produced six
findings then produced one now.

**This is a closure round, not a redesign.** v3's structure is accepted. Fold the
seven findings and change nothing else. Do not restructure the seam, do not
re-derive the module layout, do not revisit settled decisions. If a finding
tempts you toward a structural change, prefer the smallest local fix that closes
it.

Produce the **complete revised A2a task spec v4** between the extraction
markers, nothing outside them.

### Environment facts

Your working tree is at `c637e493`, the base commit, and the repository is
authoritative. Read the code; where you assert a fact you may be asked which
file and line you read it from.

You cannot read anything outside the repository — no `~`, no `$HOME`, no
installed templates — and the spec must never name a path outside the
repository, because the implementer runs in a container with only the code tree
mounted.

---

## The one gating finding

### G1 (BLOCKER) — the staged handoff cannot attest its own final bytes

`git diff --cached --check` passes, then writing that result into the staged
handoff changes the very bytes that were checked. This is the residual of the
self-naming-SHA problem you already solved with the two-commit protocol.

It is **satisfiable**, so do not treat it as a contradiction to disclaim. Adopt
this protocol and state it exactly:

1. Run a **provisional** `git diff --cached --check` on the staged handoff.
2. Record that provisional result in the handoff.
3. Restage.
4. Run a **final** `git diff --cached --check` with **no subsequent edit** of any
   kind.
5. Commit. The final check covers the final bytes; only its result is
   unrecorded, and the handoff must say so explicitly rather than imply that
   every result it contains was self-verified.

Keep the existing `git diff --check <base>..<candidate>` requirement for the
implementation-candidate commit; G1 concerns only the staged handoff.

---

## Operator-corrected finding — the review's suggested fix is measurably wrong

### G5 (MAJOR) — the discarded rich outcome trips `dead_code`, and BOTH suggested remedies fail

The finding is real: production discards `ExactScanOutcomeV1`, so its fields are
never read outside tests while `-D warnings` is mandatory.

The review suggested either "enumerate exact temporary field-scoped allowances"
or "require an explicit non-behavioral production destructure/discard."
**Both were compiled under the pinned 1.94.0 toolchain and both fail:**

| Model | Result |
|---|---:|
| `pub(super)` fields, production discards via `let _outcome = …` | **exit 1** — `error: fields … are never read` |
| the same, plus a `#[cfg(test)]` reader, built without `--test` | **exit 1** — identical error |
| explicit destructure-discard `{ rows: _, iterator_errors: _, … }` | **exit 1** — identical error |
| **private fields + `pub(super)` consuming accessor** | **exit 0** |

A `#[cfg(test)]` reader does not satisfy `dead_code` in a non-test build, and a
destructure-to-underscore does not count as a read.

**Resolve G5 and G6 together with the accessor design** — which is G6's own
suggestion, so this is one change closing two findings:

- make `CheckedScanRowV1`, `CheckedScanCompletedV1`, and `ExactScanOutcomeV1`
  **opaque to the parent**: private fields, `pub(super)` consuming accessors
  (`into_action_rows`, `into_exact_parts`, read-only row accessors as needed);
- production consumes the outcome through an accessor rather than discarding a
  struct with unread fields;
- **no new `dead_code` allowance is needed for these types.** Do not add one.

This also removes the parent's ability to fabricate engine results, which is
exactly what G6 objects to.

Re-check the six existing field-scoped allowances after this change: allowances
that are no longer needed must be removed, and the acceptance criterion stating
their count must match whatever remains. If the count changes, say so.

---

## Remaining findings to fold

### G6 (MAJOR) — construction authority

Making every field of `CheckedScanRowV1` and `CheckedScanCompletedV1`
`pub(super)` lets `sweep.rs` fabricate engine results, leaving engine-only
construction enforced by audit alone. **Closed by the G5 accessor design above.**
State the resulting construction rule once: the engine is the only constructor,
enforced by module privacy rather than by audit.

### G2 (MAJOR) — the classifier is unpinned, and the base rule is Unix-only

Measured at `crates/bridge-worktree/src/custody.rs:694`:

```rust
pub fn is_custody_record_name(path: &str) -> bool {
    let Some(stem) = path.strip_suffix(CUSTODY_RECORD_SUFFIX) else {
        return false;
    };
    !stem.is_empty() && !stem.ends_with('/')
}
```

It takes a **full path**, and its empty-basename guard tests `'/'` only. On a
backslash-spelled path, `dir\.custody.v1.json` leaves stem `dir\`, which does not
end with `'/'`, so the guard passes — where the Unix spelling
`dir/.custody.v1.json` correctly fails. Classifying an exact basename and
applying this rule to a lossy joined display path therefore diverge.

Pin the classifier: state its exact input (the full lossy joined display path),
require legacy-first / custody-second precedence, require direct delegation to
`is_custody_record_name` rather than a reimplementation, and characterize both
the backslash case and the `/.custody.v1.json` empty-stem boundary.

A2a **preserves** this behavior — it does not fix the Unix-only guard. Record
the divergence as characterized existing behavior and, if you judge it a latent
defect, note it in `### Deferred` rather than changing it here.

### G3 (MAJOR) — "unchanged decisions" does not pin the decision mapping

Selection equivalence can hold while decision behavior regresses. Add an outcome
matrix requiring, for both record kinds:

- `BothAbsent` → `Authorized`;
- `TargetPresent`, `RegisteredButAbsent`, and probe `Err` → `Refused`;
- unreadable custody → refusal with **zero** probe calls.

Verify each row against the base before pinning it; if any mapping differs from
the base, the falsification license applies.

### G4 (MAJOR) — the event's record field is not bound to the stored row

`decision` is bound to the stored projection row, but `record = path` is not
explicitly bound to that row's stored `record_path`, so a reconstruction could
emit divergent event text while the assertion still passes.

Require the event's record local to borrow the just-constructed row's
`checked.record_path`, and include that exact binding in the source audit.

### G7 (MAJOR) — pre-edit checks leave no durable evidence

Factual-anchor validation, complete Spec Ref re-reading, and row estimates are
all mandatory before editing, but nothing records them, so the mandatory stop
condition cannot be verified afterward.

Add a pre-edit checkpoint recorded in the handoff: base and clean-tree identity,
each factual anchor's disposition, the revised per-row estimates, and the
explicit proceed-or-stop decision. This is evidence of a decision already
required, not a new gate.

---

## Standing constraints

**Formatting.** Any Rust text the spec declares normative must pass
`rustfmt --check --edition 2021` under the pinned 1.94.0 toolchain **exactly as
written**. The operator verifies every block mechanically each round.

This has now failed three rounds running on the same declaration. In v1, v2 and
v3 you emitted:

```
    fn read_legacy(
        &self,
        enumerated_name: &OsStr,
        record_display: &str,
    ) -> Option<WorktreeSidecar>;
```

and rustfmt requires exactly:

```
    fn read_legacy(&self, enumerated_name: &OsStr, record_display: &str)
        -> Option<WorktreeSidecar>;
```

The single-line form is 100 characters, at `max_width`, so rustfmt wraps the
return type onto a continuation line rather than splitting the parameters. In v3
`assert_effective_item_type` had the same defect and belongs on one line at 89
characters. **Emit both in the rustfmt form above.** Do not hand-split parameter
lists for readability anywhere in normative Rust.

**Sizing.** Keep the deterministic counted-line metric. v3 estimated 665 against
a 775 cap. The accessor change is roughly neutral — accessors add lines, removed
allowances subtract them — and G7's checkpoint adds a few. Re-derive honestly;
if the total moves, say so and justify it rather than compressing evidence.

**Do not change** anything not named in this document: the settled A2a/A2b
boundary, the removal of attestation, the public API preservation, the
repository-hygiene guard at both commit points, the owner-approved inline
handoff schema, the `### Deferred` section, or the falsification license.

## Output contract

Emit the complete A2a task spec v4 between the markers, with:

- the same front matter (`task-type: implement`);
- `## Description`, `## Acceptance Criteria`, `## Files`, `## Spec Refs`,
  `## Commit Message`;
- all seven findings folded, each to one answer;
- `### Deferred` carried forward, plus the Unix-only classifier note if you
  judge it a latent defect;
- the falsification license carried forward;
- no path outside the repository anywhere in the document.

---

## Reference — A2a spec v3, verbatim

Revise the document below. Reproduce heading levels as they appear. This
reference ends at the end of the document.


# R2f1b 3d T3a increment 1, slice A2a v3 — production-bound compatibility scan engine

## Description

Implement slice A2a against exact base commit `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`.

This v3 specification is the single disclosed owner-authorized round beyond the original two-round review cap. The loop is converging: every round-1 finding was adjudicated fixed and none was re-reported. Continue from v2; do not restart or replace the partially reviewed artifact.

Direct read-only inspection of the clean repository tree at the base confirmed:

- `sweep_orphans_with_exact_absence` returns `()`, canonicalizes the supplied root with `canonicalize_lenient`, drains `scan_worktree_records`, computes the existing decisions, and emits the existing per-row event;
- `scan_worktree_records` eagerly returns `Vec<(String, ScannedWorktreeRecordV1)>`;
- that scanner calls `std::fs::read_dir` before attempting `PinnedDirectoryV1::open`;
- a `read_dir` failure returns an empty vector;
- iterator-item errors are flattened;
- malformed or unreadable legacy sidecars are silently omitted through `read_sidecar`;
- custody-named entries remain represented as decoded custody records or `UnreadableCustody`;
- custody pin failure preserves legacy reads and yields
  `CustodyReadRefusalV1::Unreadable("sweep root is not pinnable".to_string())`
  for custody names;
- the exact-absence route passes its lenient-canonical root to the scanner;
- `sweep_orphans`, `WorktreeRunEndGuard`, and the custody-lock test use the public action scanner with the raw root supplied by their caller;
- `sweep_orphans` calls `sweep_orphans_with_exact_absence` in statement position, then independently canonicalizes its action guard root;
- `EXACT_ABSENCE_POLICY_READY_V1` remains false;
- the A1 report module remains 598 lines and retains four constructor `dead_code` allowances;
- `ExactAbsenceRootRefusalV1` already contains `CannotCanonicalize` and `CannotEnumerate`;
- `UnusedCandidateDecisionV1` is `Copy`, `PartialEq`, and `Eq`;
- `crates/bridge-worktree/Cargo.toml` has exactly two dev-dependencies: `bridge-coordinator` and `bridge-controller`;
- the pinned toolchain is Rust 1.94.0 with rustfmt, clippy, and LLVM tools;
- `PinnedDirectoryV1` exposes captured identity and descriptor-relative custody reads, while bridge-core’s descriptor-name enumerator is crate-private and unavailable to `bridge-worktree`;
- `deepest_existing_path` is defined at `crates/bridge-core/src/fs_custody.rs:1511` and installed as the production resolver at line 1777;
- the checked-in CI coverage job runs `cargo llvm-cov --workspace` on `ubuntu-latest`;
- `AGENTS.md` requires
  `cargo run -p a2a-bridge -- validate --repo-hygiene`
  before committing;
- no `handoff-template*` file exists in the repository, and `AGENTS.md` contains no requirement to read one.

These are source-tree anchors, not build or test evidence. No build, formatter, lint, or test command was run while authoring this specification.

Before editing, verify the exact base commit, require a clean worktree, re-read every authoritative file under “Spec Refs,” and complete the pre-edit sizing check. If any factual anchor is false, apply the falsification license instead of adapting the implementation to a stale claim.

### Scope and settled boundary

A2a owns:

- one private checked-scan child module containing the compatibility source, session, mandatory engine, completed result, and deterministic injected source tests;
- one production-bound checked-scan engine used by both the exact-absence and compatibility/action projections;
- one shared display-name selection and read policy;
- a private exact-route pin-opener seam below supplied-root canonicalization;
- one deliberate `checked_scan.rs` → `sweep.rs` seam whose complete types, fields, visibility, construction rules, and projection signatures are pinned below;
- production-used action and exact result-to-projection interfaces that accept the engine’s `Result`, including source-open refusal handling;
- deterministic injected-source equivalence evidence that invokes those production projection interfaces rather than reimplementing them in tests;
- a decision-bearing rich exact outcome retaining the canonical root, iterator-error count, root observations, selected rows, exact names, decoded or refused records, and production-computed decisions;
- preservation of the public action scanner’s eager vector projection and raw-root behavior;
- preservation of the public exact-absence function’s unit return and current assessment and event behavior;
- the decision about enumeration-descriptor ownership on the A2a/A2b boundary;
- external compile-time assertions for the visibility and complete signatures of both public scan functions;
- the exact `effective()` iterator item-type assertion;
- structural, mechanism, characterization, injected conformance, event-source-audit, and compiler evidence for the refactor;
- an interim A2a handoff durably recording the exact implementation commit, source audit, pre-commit guards, gate evidence, exclusions, whitespace evidence, and final counted-line worksheet.

A2a does not:

- change the public signature or return type of `sweep_orphans_with_exact_absence`;
- populate an `ExactAbsenceSweepReportV1`;
- change any public report type, accessor, iterator, readiness rule, or constructor allowance;
- add report refusal, requested-root, canonical-root, iterator-status, entry, root-observation, or effective-entry behavior;
- add eager report assessment or new tracing behavior;
- add scoped tracing capture or a `tracing-subscriber` dependency;
- add any filesystem observation, filesystem-attestation utility, platform-labelled fixture gate, or new dependency;
- populate authoritative root captures in production;
- characterize registration-path UTF-8 behavior;
- perform the final mutation audit or final combined A2 handoff;
- add the increment-2 population-admission rule;
- set `EXACT_ABSENCE_POLICY_READY_V1` to true;
- construct `IneligiblePopulation` or `CannotConstructSubject` production assessments;
- add ownership, locking, transition, publication, settlement, unlink, removal, prune, rename, backend-cleanup, or T3b authority;
- change CLI behavior;
- claim that a scan result is action authority.

A2b starts from the accepted two-commit A2a tip. It consumes the rich exact outcome for report return and population, adds eager assessment and scoped tracing evidence, replaces the compatibility enumerator where required to obtain retained enumeration-descriptor evidence, adds birthtime-capability evidence and UTF-8 characterization, runs the complete platform matrix and mutation audit, and produces the final combined A2 handoff.

T3a decides and reports. T3b will independently re-open, re-read, re-bind, re-apply admission, re-prove exact absence, and retain its own lock and action-time authority through any later effect.

### Filesystem evidence posture

A2a makes no new claim about filesystem identity, mount identity, filesystem type, or real-directory traversal ordering. It adds no filesystem observation and preserves the existing `read_dir`, `read_sidecar`, `read_custody_record_in`, and custody pin-open semantics exactly. Genuine runtime red is forbidden because both public behaviors and return types remain unchanged.

A2a’s projection-equivalence claim is proved with injected deterministic sources. Those sources can force exact ordered name streams, source-open refusal, pin failure, malformed or unreadable legacy input, every required custody refusal, iterator errors, exact non-UTF-8 names, and non-default root observations. The injected sessions run through the real engine and then through the same production result-to-projection functions used by the filesystem wrappers.

The checked-in CI coverage lane runs the workspace suite on Ubuntu, and the operator’s established host gate runs the same suite on macOS. When those existing lanes execute the accepted A2a tree, the unchanged compatibility source is exercised on their respective filesystems. Those executions are ordinary suite evidence, not bespoke A2a filesystem attestations.

Attested real-filesystem conformance is a separate future slice sequenced with A2b’s platform matrix, where the birthtime-capability question already belongs. That slice must not be inferred complete from A2a’s deterministic projection evidence.

Two independent real `read_dir` traversals have no specified relative order. A2a therefore has no ordered-equality oracle across real traversals. It proves exact order preservation only against injected deterministic name streams.

### Public API preservation

The public exact-absence declaration remains exactly:

```rust
pub fn sweep_orphans_with_exact_absence(root: &str, probe: &dyn ExactAbsenceProbeV1)
```

It continues to return `()`. Do not add a report-returning overload, compatibility wrapper, new public scanner, or public test seam.

The public action scanner remains exactly:

```rust
pub fn scan_worktree_records(root: &str) -> Vec<(String, ScannedWorktreeRecordV1)>
```

Every existing caller continues to compile without source changes unless an import or internal routing change is mechanically necessary. The fifteen report types, `effective()` iterator, false readiness gate, four A1 constructor allowances, and stale A1-to-A2 constructor comments remain untouched for A2b.

Add this external integration-test assertion in `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs` so both `pub` visibility and the complete function types are compiler-enforced from outside the crate:

```rust
#[test]
fn public_scan_functions_keep_visibility_and_exact_signatures() {
    let _: fn(&str) -> Vec<(String, ScannedWorktreeRecordV1)> = scan_worktree_records;
    let _: fn(&str, &dyn ExactAbsenceProbeV1) = sweep_orphans_with_exact_absence;
}
```

Import the four referenced public names through `bridge_worktree::sweep`. An accidental visibility reduction or signature change must fail compilation of the integration-test crate.

Replace the existing untyped `let _ = report.effective();` check with this exact test-only item-type assertion, leaving production report code unchanged:

```rust
fn assert_effective_item_type<'a>(_: impl Iterator<Item = &'a ExactAbsenceSweepEntryV1>) {}
```

Within `assert_public_accessor_signatures`, invoke it as:

```rust
assert_effective_item_type(report.effective());
```

Because A2a adds no public break and no new observable behavior, it has no genuine behavioral-red test against the untouched base. Its evidence is base-green characterization, new-private-seam mechanism evidence, production-route structural evidence, injected conformance evidence, event-source-audit evidence, evidence-infrastructure mechanism evidence, and compiler API-preservation evidence. Do not manufacture a runtime-red claim.

### Private checked-scan seam

Create `crates/bridge-worktree/src/sweep/checked_scan.rs` and declare it as a private child module of `sweep.rs`.

The seam decision is singular:

> `checked_scan.rs` exposes to its parent only the compatibility pin-opener vocabulary, the open-refusal vocabulary, the owned selected-row and completed-result vocabulary, the root-observation vocabulary, and the production filesystem entry point. The source trait, session trait, concrete source, concrete session, iterator-entry refusal, classifier, and engine remain child-private. Tests are nested below the child, construct results only by running the real engine through the authorized test factory, and invoke the parent’s real production result-to-projection functions.

Every type named in a `pub(super)` signature or field is itself at least `pub(super)`. No child-private type may occur in a `pub(super)` signature.

Land the following declarations with these exact names, fields, field types, and visibility:

```rust
use std::ffi::{OsStr, OsString};
use std::path::Path;

use bridge_core::fs_custody::{BirthTimeV1, PinnedDirectoryV1};

use crate::custody::{CustodyReadRefusalV1, WorktreeCustodyRecordV1};
use crate::provider_path::WorktreeSidecar;

use super::ScannedWorktreeRecordV1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CheckedScanOpenRefusalV1 {
    CannotEnumerate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckedScanEntryRefusalV1 {
    CannotReadEntry,
}

trait CheckedScanSourceV1 {
    fn open(
        &self,
        enumeration_root: &Path,
    ) -> Result<Box<dyn CheckedScanRootSessionV1>, CheckedScanOpenRefusalV1>;
}

trait CheckedScanRootSessionV1 {
    fn next_name(&mut self) -> Option<Result<OsString, CheckedScanEntryRefusalV1>>;

    fn read_legacy(&self, enumerated_name: &OsStr, record_display: &str)
        -> Option<WorktreeSidecar>;

    fn read_custody(
        &self,
        enumerated_name: &OsStr,
    ) -> Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1>;

    fn finish(self: Box<Self>) -> RootObservationSetV1;
}

pub(super) trait CompatibilityPinOpenerV1 {
    fn open_pin(&self, enumeration_root: &Path) -> Option<PinnedDirectoryV1>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FilesystemCompatibilityPinOpenerV1;

impl CompatibilityPinOpenerV1 for FilesystemCompatibilityPinOpenerV1 {
    fn open_pin(&self, enumeration_root: &Path) -> Option<PinnedDirectoryV1> {
        PinnedDirectoryV1::open(enumeration_root, "worktree sweep root").ok()
    }
}

struct CompatibilityCheckedScanSourceV1<P> {
    pin_opener: P,
}

impl<P: CompatibilityPinOpenerV1> CompatibilityCheckedScanSourceV1<P> {
    const fn new(pin_opener: P) -> Self {
        Self { pin_opener }
    }
}

struct CompatibilityCheckedScanRootSessionV1 {
    names: std::fs::ReadDir,
    custody_root: Option<PinnedDirectoryV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RootIdentityCaptureV1 {
    #[allow(dead_code)]
    pub(super) dev: Option<u64>,
    #[allow(dead_code)]
    pub(super) ino: Option<u64>,
    #[allow(dead_code)]
    pub(super) birthtime: Option<BirthTimeV1>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RootObservationSetV1 {
    #[allow(dead_code)]
    pub(super) retained_enumeration_object: Option<RootIdentityCaptureV1>,
    #[allow(dead_code)]
    pub(super) pinned_custody_directory: Option<RootIdentityCaptureV1>,
    #[allow(dead_code)]
    pub(super) final_named_root: Option<RootIdentityCaptureV1>,
}

pub(super) struct CheckedScanRowV1 {
    pub(super) record_path: String,
    pub(super) enumerated_name: OsString,
    pub(super) scanned: ScannedWorktreeRecordV1,
}

pub(super) struct CheckedScanCompletedV1 {
    pub(super) rows: Vec<CheckedScanRowV1>,
    pub(super) iterator_error_count: usize,
    pub(super) root_observations: RootObservationSetV1,
}

impl CheckedScanCompletedV1 {
    pub(super) fn into_rows(self) -> Vec<(String, ScannedWorktreeRecordV1)> {
        self.rows
            .into_iter()
            .map(|row| (row.record_path, row.scanned))
            .collect()
    }
}
```

The exact engine signature is:

```rust
fn scan_checked_rows_with_source(
    enumeration_root: &Path,
    source: &dyn CheckedScanSourceV1,
) -> Result<CheckedScanCompletedV1, CheckedScanOpenRefusalV1>
```

The exact production filesystem entry signature is:

```rust
pub(super) fn scan_compatibility_with_pin_opener<P>(
    enumeration_root: &Path,
    pin_opener: P,
) -> Result<CheckedScanCompletedV1, CheckedScanOpenRefusalV1>
where
    P: CompatibilityPinOpenerV1,
```

The only authorized test-only completed-result construction path is:

```rust
#[cfg(test)]
fn scan_checked_rows_for_test(
    enumeration_root: &Path,
    source: &dyn CheckedScanSourceV1,
) -> Result<CheckedScanCompletedV1, CheckedScanOpenRefusalV1>
```

Its body delegates directly to `scan_checked_rows_with_source` and contains no selection, read, status, observation, or projection logic. Injected tests define child-private scripted implementations of `CheckedScanSourceV1` and `CheckedScanRootSessionV1`, call `scan_checked_rows_for_test`, and pass the returned `Result` to the production projection functions in `sweep.rs`. They must not construct `CheckedScanCompletedV1` or `CheckedScanRowV1` with literals and must not duplicate either production projection.

In the non-test region, only `scan_checked_rows_with_source` may construct `CheckedScanRowV1` or `CheckedScanCompletedV1`. The parent may destructure a successful completed result but may not construct one.

Do not place `#[allow(dead_code)]` on the module, a type, an `impl`, or a function. Within `checked_scan.rs`, the only allowances are the six field-scoped attributes shown above. The four existing constructor allowances in `sweep/report.rs` remain additional, unchanged allowances and are outside this `checked_scan.rs` constraint.

A2a intentionally omits the earlier block’s unused `CustodyRootObservationV1` import, `complete_identity`, and `classify_root_observations`. A2b adds declarations only when it consumes them for report population. A2a carries `RootIdentityCaptureV1` and `RootObservationSetV1` because the session’s `finish`, the completed engine result, the rich exact outcome, and the non-default-observation projection tests consume that boundary.

Every normative Rust block in this specification must pass
`cargo fmt --all -- --check`
under the pinned Rust 1.94.0 toolchain after landing. Re-derive formatting with that toolchain. In particular, do not preserve a hand split for a signature rustfmt places on one line, and do not preserve a one-line signature rustfmt wraps. A formatting assertion without a measurement is not authority to alter an already clean block.

### Compatibility source

Implement `CompatibilityCheckedScanSourceV1<P>` with the current filesystem policy and parameterize only custody pin opening.

Its A2a open sequence is exact:

1. Call `std::fs::read_dir(enumeration_root)`.
2. Return `CheckedScanOpenRefusalV1::CannotEnumerate` only if that call fails.
3. After successful `read_dir`, call `open_pin(enumeration_root)`.
4. Retain the `ReadDir` and returned `Option<PinnedDirectoryV1>` in `CompatibilityCheckedScanRootSessionV1`.
5. Permit legacy reads through path-based `read_sidecar` regardless of pin outcome.
6. With no custody pin, return
   `CustodyReadRefusalV1::Unreadable("sweep root is not pinnable".to_string())`
   for every custody read.
7. Consume the session in `finish` and return `RootObservationSetV1::default()`.

A pin failure is not a source-open failure. A readable directory with a failed custody pin must still enumerate completely, preserve valid legacy rows, and retain every custody-named row with the exact not-pinnable refusal.

`next_name` maps each successful `ReadDir` item to its exact `OsString`, maps each iterator error to `CheckedScanEntryRefusalV1::CannotReadEntry`, and returns `None` only when enumeration ends. It must not reconstruct an exact name from lossy display text.

`read_legacy` calls the existing `read_sidecar(record_display)`. `read_custody` calls the existing `read_custody_record_in` with the retained custody pin and exact enumerated name. No new legacy decoder, custody decoder, selection vocabulary, or read policy is permitted.

### Shared selection and mandatory engine

The engine borrows the source only for `open`, owns the returned boxed session, and returns one owned completed result. It performs no canonicalization and passes `CheckedScanOpenRefusalV1::CannotEnumerate` through unchanged.

Its protocol is exact:

1. Call `source.open(enumeration_root)`.
2. Repeatedly call `next_name`.
3. Count an `Err` item in `iterator_error_count: usize` and continue.
4. For each successful name, retain the exact `OsString`.
5. Construct the lossy full display path by joining `enumeration_root` with that exact name.
6. Apply one shared classifier preserving the base legacy suffix and custody-name rules.
7. Immediately perform the selected legacy or custody read before requesting the next name.
8. Silently omit a legacy row when `read_sidecar` returns `None`.
9. Retain decoded custody and custody-refusal rows in injected or filesystem enumeration order.
10. Ignore unrelated names.
11. Call `finish` exactly once after `next_name` returns `None`.
12. Construct the completed result with the ordered rows, `usize` iterator-error count, and returned observation set.

A custody read or decode refusal is a retained row, not an iterator error. A malformed or unreadable legacy sidecar remains silently omitted and contributes neither a row nor an iterator error.

`scan_compatibility_with_pin_opener` constructs `CompatibilityCheckedScanSourceV1`, invokes `scan_checked_rows_with_source`, and does nothing else.

No production code outside `checked_scan.rs` can name `CheckedScanSourceV1`, `CheckedScanRootSessionV1`, `CheckedScanEntryRefusalV1`, `CompatibilityCheckedScanSourceV1`, or `CompatibilityCheckedScanRootSessionV1`. Within the non-test portion of `checked_scan.rs`, `scan_checked_rows_with_source` is the only function permitted to call `next_name`, `read_legacy`, `read_custody`, or `finish`.

### Production result projections and rich exact outcome

Define the following private parent-module types in `sweep.rs`:

```rust
struct ExactScanProjectionRowV1 {
    checked: CheckedScanRowV1,
    decision: UnusedCandidateDecisionV1,
}

struct ExactScanCompleteV1 {
    canonical_root: SessionCwd,
    iterator_error_count: usize,
    root_observations: RootObservationSetV1,
    rows: Vec<ExactScanProjectionRowV1>,
}

enum ExactScanOutcomeV1 {
    Refused {
        canonical_root: Option<SessionCwd>,
        refusal: ExactAbsenceRootRefusalV1,
    },
    Complete(ExactScanCompleteV1),
}
```

Do not add `ExactScanProjectionRefusalV1` or any other duplicate refusal enum. `ExactScanOutcomeV1::Refused` always uses the existing `ExactAbsenceRootRefusalV1` vocabulary:

- canonicalization failure carries `canonical_root: None` and
  `ExactAbsenceRootRefusalV1::CannotCanonicalize`;
- source-open failure after successful canonicalization carries
  `canonical_root: Some(canonical_root)` and
  `ExactAbsenceRootRefusalV1::CannotEnumerate`.

The canonical root observed before an enumeration refusal therefore survives for A2b.

Add these two production-used result-to-projection interfaces:

```rust
fn project_action_scan_result(
    result: Result<CheckedScanCompletedV1, CheckedScanOpenRefusalV1>,
) -> Vec<(String, ScannedWorktreeRecordV1)>
```

```rust
fn project_exact_scan_result(
    canonical_root: SessionCwd,
    result: Result<CheckedScanCompletedV1, CheckedScanOpenRefusalV1>,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactScanOutcomeV1
```

Both filesystem wrappers and injected conformance tests must call these functions. Tests must not restate their mappings.

`project_action_scan_result` is the only metadata-erasing projection:

- `Err(CheckedScanOpenRefusalV1::CannotEnumerate)` becomes an empty vector;
- `Ok(completed)` is consumed through the exact
  `CheckedScanCompletedV1::into_rows(self) -> Vec<(String, ScannedWorktreeRecordV1)>`
  signature;
- exact names, iterator status, and root observations are erased only here.

`project_exact_scan_result` never calls `into_rows`. Its behavior is exact:

1. Map `CheckedScanOpenRefusalV1::CannotEnumerate` to `ExactScanOutcomeV1::Refused` while retaining the supplied canonical root.
2. On success, destructure the completed result into ordered checked rows, `iterator_error_count`, and `root_observations`.
3. Only after the engine has completed enumeration, all selected reads, terminal `None`, and `finish`, iterate the checked rows in order.
4. Compute each row’s decision once with the existing legacy, custody, or unreadable-custody assessment path.
5. Construct `ExactScanProjectionRowV1` with that checked row and the production-computed decision.
6. After constructing that decision-bearing row, emit exactly one existing event for it, without a condition or second emitter:
   `tracing::info!(record = path, ?decision, "made exact-absence decision");`.
7. Bind the event’s `decision` local from the just-constructed
   `ExactScanProjectionRowV1.decision`; the event and retained decision must not be computed independently.
8. Return `ExactScanOutcomeV1::Complete` containing the canonical root, exact `usize` iterator-error count, full root observations, and all decision-bearing rows.

The rich exact outcome is private, is not a report, grants no action authority, and is discarded by the public A2a wrapper. It exists so A2a can prove the unchanged decisions and so A2b can consume the already-observed canonical root, iterator status, root observations, exact names, selected records, and decisions without replacing this seam or re-observing mutable filesystem state.

### Production wrappers and refusal mappings

Add the private action helper with this complete signature:

```rust
fn scan_worktree_records_with_pin_opener<P>(
    root: &str,
    pin_opener: P,
) -> Vec<(String, ScannedWorktreeRecordV1)>
where
    P: CompatibilityPinOpenerV1,
```

It:

- passes `Path::new(root)` directly to `checked_scan::scan_compatibility_with_pin_opener`;
- passes the returned `Result` directly to `project_action_scan_result`;
- performs no canonicalization and contains no separate refusal or row-projection logic.

Production `scan_worktree_records` delegates to this helper with `FilesystemCompatibilityPinOpenerV1`.

Add the exact-route helper with this complete signature:

```rust
fn sweep_orphans_with_exact_absence_with_pin_opener<P>(
    root: &str,
    probe: &dyn ExactAbsenceProbeV1,
    pin_opener: P,
) -> ExactScanOutcomeV1
where
    P: CompatibilityPinOpenerV1,
```

Its order and mappings are fixed:

1. Invoke `canonicalize_lenient(root)`.
2. On failure, return `ExactScanOutcomeV1::Refused` with `canonical_root: None` and
   `ExactAbsenceRootRefusalV1::CannotCanonicalize`, without consulting the opener or source.
3. After successful canonicalization, call
   `checked_scan::scan_compatibility_with_pin_opener`
   with the canonical root and supplied opener.
4. Pass the canonical root, returned engine `Result`, and probe directly to
   `project_exact_scan_result`.
5. Do not restate source-open mapping, decision computation, event emission, or outcome construction in the wrapper.

Production `sweep_orphans_with_exact_absence` evaluates this helper with `FilesystemCompatibilityPinOpenerV1`, discards the private outcome explicitly, and returns `()`.

### Preserved action projection

The public action scanner retains every existing observable:

- the exact public signature and eager vector return;
- raw-root `read_dir` and raw-root pin opening;
- no canonicalization of its enumeration argument;
- source-open failure producing an empty vector;
- iterator-item errors being flattened from the public vector;
- legacy reads using the lossy full display path;
- malformed or unreadable legacy sidecars being omitted;
- custody selection using `is_custody_record_name`;
- custody reads using the exact enumerated `OsString`;
- decoded custody records retaining full structural equality;
- custody refusals retaining their exact variants and messages;
- pin failure preserving legacy rows and refusing custody rows as not pinnable;
- source enumeration order.

Existing consumers of `scan_worktree_records`, including the run-end guard and custody-lock coverage, continue to use only this public projection.

### Preserved exact/action root separation

The exact-absence route continues to canonicalize the supplied root before enumeration. The action scanner continues to enumerate the caller’s raw root spelling.

`sweep_orphans` remains behaviorally equivalent to:

```rust
sweep_orphans_with_exact_absence(
    root,
    &crate::host_git::HostGitWorktree::new(),
);
let Ok(root_cwd) = canonicalize_lenient(root) else {
    tracing::warn!(root, "skipping worktree sweep with non-canonical root");
    return;
};
for (path, scanned) in scan_worktree_records(root) {
    // Existing compatibility/action behavior remains unchanged.
}
```

A2a does not extract or alter the action phase, guard warning, removal decisions, custody classification, locking, or cleanup behavior.

The refactor must preserve the eager ordering present at the base: the engine drains enumeration and performs selected reads before the exact route assesses or logs any row. An injected operation log must prove that no probe assessment occurs before `next_name` returns `None` and `finish` completes.

### Injected deterministic conformance matrix

Place the injected source, injected session, operation log, and matrix tests in the nested `checked_scan.rs` test module. This location permits the tests to implement the child-private source/session traits and to invoke the parent’s private production projections without widening production visibility.

The full matrix is driven by two equivalent injected source sessions with the same explicitly ordered stream. Each session runs through `scan_checked_rows_for_test`. One returned result passes to `project_action_scan_result`; the other passes to `project_exact_scan_result`. No test helper may duplicate selection, refusal mapping, metadata projection, decision computation, or event computation.

Compare ordered projections as follows:

| Injected condition | Required evidence |
|---|---|
| Valid matching legacy sidecar | Both projections select the same display path and structurally equal `WorktreeSidecar`. |
| Malformed or unreadable legacy sidecar | `read_legacy` returns `None`; both projections omit the name. |
| Valid custody record | Both projections select the same display path and retain structurally equal decoded `WorktreeCustodyRecordV1` values before exact assessment. |
| Malformed custody record | Both projections retain the same `CustodyReadRefusalV1::Decode` value. |
| Over-bound custody record | Both projections retain `CustodyReadRefusalV1::OverBound`. |
| Multiply-linked custody record | Both projections retain `CustodyReadRefusalV1::MultiLink`. |
| Unreadable custody record | Both projections retain the same exact `CustodyReadRefusalV1::Unreadable` value, and the exact row retains a production-computed `Refused` decision. |
| Unrelated name | Both projections omit it. |
| Pin failure with valid legacy and custody names | Both preserve the legacy row and retain the custody row with the exact not-pinnable refusal. |
| `Ok, Err, Ok, Err` name stream | Both retain the two successful selected rows in injected order; the completed result records `iterator_error_count == 2usize`; the rich exact outcome retains that count. |
| Exact non-UTF-8 name on Unix | The engine and rich exact row retain the original `OsString`; no reconstruction from display text occurs. |
| Non-default root observations | The action vector is unchanged; the rich exact outcome retains the exact non-default observation set; its ordered records and production-computed decisions equal the default-observation control. |
| Source-open refusal | The action projection is empty; the exact projection returns `Refused` with `CannotEnumerate` and the supplied canonical root; it performs zero assessments and produces no decision-bearing rows. |

The scripted session records every `next_name`, selected read, `finish`, and probe-assessment operation. Its expected log is literal: each selected read follows its corresponding successful name before the next name request; `finish` follows terminal `None` exactly once; all exact assessments follow `finish`.

The unchanged-decision evidence must inspect `ExactScanProjectionRowV1.decision` values produced by `project_exact_scan_result`. Probe logs alone, selected rows alone, and a duplicated test-side decision function are inadmissible substitutes.

Do not create a real-filesystem ordering comparison, infer one traversal’s order from another, or label injected entry-kind refusals as filesystem-attested observations.

The compatibility-source tests remain responsible for proving that the real source calls `read_dir` before the pin opener, preserves legacy reads on opener failure, uses descriptor-relative custody reads when pinned, and emits the exact not-pinnable refusal when not pinned.

### Enumeration-descriptor ownership decision

A2a deliberately retains `std::fs::ReadDir` in the compatibility session to preserve behavior. It does not claim that `ReadDir` exposes an inspectable identity for the directory object being enumerated.

The meaning of `RootObservationSetV1::retained_enumeration_object` remains:

> The field may contain an identity only when it was captured from the exact retained directory descriptor whose duplicated descriptor drives name enumeration. Identity read from the root path, from the separate custody pin, or from a descriptor that did not drive enumeration does not satisfy the field.

Accordingly:

- A2a production leaves `retained_enumeration_object` as `None`;
- A2a production leaves all three root observations as `None`;
- A2a production `finish` returns `RootObservationSetV1::default()`;
- A2a does not present `std::fs::ReadDir` as retained identity evidence;
- A2a does not populate the field from `PinnedDirectoryV1`, because that is the separate custody-read descriptor;
- A2a does not weaken the field to mean path metadata observed near enumeration;
- the rich exact outcome retains the default observations now so A2b can populate the same boundary later without replacing the projection seam.

A2b must replace A2a’s `ReadDir` storage on the required Unix lanes with a bridge-core retained-directory enumerator that:

- opens and retains one directory descriptor independently of the custody pin opener;
- enumerates names from a duplicate of that same descriptor;
- exposes metadata from the retained descriptor for `retained_enumeration_object`;
- preserves independent custody pin-failure behavior;
- preserves raw-root alias acceptance for the action projection;
- leaves the observation unavailable on a target where descriptor-owned enumeration cannot be provided without changing scan behavior.

A2b must reserve a distinct 140-counted-line worksheet row for that bridge-core enumerator, worktree integration, and focused tests. That budget is not part of A2a and may not be borrowed to extend A2a.

### Tracing infrastructure and event correctness

A2a adds no tracing capture and changes no event schema. Do not add `tracing-subscriber`, a public reporter, or a global test subscriber.

The existing exact-absence event remains exactly:

```rust
tracing::info!(record = path, ?decision, "made exact-absence decision");
```

Event correctness is established through a mandatory source audit, not merely preservation of the literal. The non-test production source must contain exactly one unguarded call site for this exact event. That call site must be:

- inside the post-`finish` loop over every retained checked row;
- after the row’s decision has been computed and stored in `ExactScanProjectionRowV1`;
- driven from that stored decision;
- executed exactly once per retained row;
- absent from helpers, refusal branches, test code, and any second emitter.

The handoff source audit must provide file-and-line evidence for the single call site, the enclosing loop, the preceding decision-bearing row construction, and the absence of another emitter. A duplicate, conditional, or pre-assessment emitter fails acceptance.

A2b owns scoped, panic-safe tracing capture when it adds report population and event-order evidence. A2b must explicitly authorize any required test dependency or shared test utility at that time.

A2a adds no dependency. `crates/bridge-worktree/Cargo.toml` remains byte-for-byte unchanged, its dev-dependency set remains exactly `bridge-coordinator` and `bridge-controller`, and `Cargo.lock` remains untouched.

### Required tests and evidence classification

Use these test names or equally specific names preserving the stated evidence:

| Required test | Category | Evidence against untouched `c637e493` | Production or evidence-infrastructure mutation caught |
|---|---|---|---|
| `compatibility_open_refusal_never_calls_pin_opener` | New-private-seam mechanism | The seam does not compile on the base | Calling the custody pin opener before successful `read_dir`, or treating pin failure as source-open failure |
| `compatibility_pin_failure_preserves_legacy_and_refuses_custody` | New-private-seam compatibility | The opener seam does not exist on the base | Suppressing all rows on pin failure, refusing legacy reads, or losing the exact not-pinnable refusal |
| `checked_scan_reads_each_selected_name_before_next_and_finishes_once` | New-private-seam ordering | The engine does not exist on the base | Prefetching the next name, duplicating or skipping `finish`, or reading outside the engine |
| `checked_scan_counts_iterator_errors_and_continues_in_injected_order` | New-private-seam status | The injected source does not compile on the base | Stopping at the first item error, using a non-`usize` count, counting a custody refusal as an iterator error, or reordering successful rows |
| `checked_scan_silently_omits_bad_legacy_and_retains_bad_custody` | Refactor characterization | Equivalent base behavior is green | Emitting malformed legacy rows, dropping custody refusals, or coupling decode refusal to iterator status |
| `report_side_pin_failure_uses_post_canonicalization_opener_seam` | New-private-seam routing | The helper does not exist on the base | Consulting the opener before canonicalization or hardcoding the production opener inside the helper |
| `scan_worktree_records_preserves_raw_root_and_public_projection` | Base-green characterization | Existing behavior is green on the base | Canonicalizing the action root, changing the vector shape, or exposing iterator/root details publicly |
| `exact_route_preserves_canonical_scan_root_and_unit_return` | Base-green characterization and compiler evidence | Existing behavior is green on the base | Passing the raw root to exact enumeration or changing the public return type |
| `injected_sources_use_production_action_and_exact_projections` | Injected refactor conformance | No genuine runtime red; the injected seam is absent on the base | Bypassing either production result projection or duplicating projection logic in tests |
| `injected_sources_prove_action_and_exact_projection_equivalence` | Injected refactor conformance | No genuine runtime red; the injected seam is absent on the base | Drifting selection, omission, decoded custody content, refusal classification, injected order, or pin-failure policy |
| `exact_projection_retains_production_computed_decisions` | Evidence-infrastructure mechanism | The decision-bearing private outcome is absent on the base | Assigning a wrong decision while preserving selected rows and probe activity |
| `checked_scan_retains_exact_non_utf8_name_internally` | New-private-seam exact-name evidence | The engine does not exist on the base | Reconstructing the exact name from lossy display text |
| `nondefault_root_observations_survive_exact_without_changing_rows_or_decisions` | Evidence-infrastructure mechanism | The rich exact outcome is absent on the base | Erasing root observations, gating or reordering exact rows based on them, or changing production decisions |
| `enumeration_refusal_retains_canonical_root_and_skips_assessment` | Evidence-infrastructure mechanism | The rich refusal outcome is absent on the base | Dropping the canonical root after successful canonicalization, or assessing rows after source-open refusal |
| `action_projection_erases_only_action_metadata` | Evidence-infrastructure mechanism | The completed-result projection seam is absent on the base | Leaking exact names/status/observations publicly or erasing them before the exact projection |
| `public_scan_functions_keep_visibility_and_exact_signatures` | Compiler API preservation | Base signatures compile; the new external assertion does not exist | Reducing either function’s visibility or changing either complete function type |
| Exact `effective()` iterator item assertion | Compiler API preservation | The production iterator is already green | Changing the item from `&ExactAbsenceSweepEntryV1` while leaving a merely callable iterator |
| Existing sweep and custody-lock tests | Existing base-green regression | Existing behavior is green | Changing legacy deletion, V3 protection, run-end handling, custody locking, or public scanner consumers |

Every new test must state its evidence category and the production or evidence-infrastructure mutation it catches. Evidence-infrastructure tests are not required to invent a production mutation.

The injected suite must additionally cover:

- source-open refusal;
- zero pin calls on real compatibility source-open refusal;
- complete enumeration;
- literal `Ok, Err, Ok, Err` ordering;
- `usize` iterator-error count equal to two;
- ignored names;
- malformed and unreadable legacy omission;
- custody refusal retention without increasing iterator errors;
- decoded custody structural equality before assessment;
- exact non-UTF-8 name retention on Unix;
- selected read before the next name request;
- `finish` after terminal `None`;
- `finish` exactly once;
- default and non-default root observations;
- action metadata erasure;
- exact metadata retention;
- production-computed decision retention;
- canonicalization refusal before opener consultation;
- canonical-root retention on source-open refusal;
- source-open refusal before assessment;
- both public functions retaining their existing return types.

No A2a test may be labelled genuine runtime red.

### Gates, pre-commit guards, and single completion rule

The mandatory acceptance gates are:

- `cargo fmt --all -- --check`;
- `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings`;
- `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast`;
- `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast`.

All four commands must exit zero against the exact implementation-candidate commit. The full and package suites must report passed, failed, ignored, measured, and filtered totals, plus the number of test binaries and doc-test suites. Do not double-count nested or filtered subprocess output.

This is the only completion rule: a missing, blocked, killed, incomplete, or red mandatory gate leaves A2a pending and cannot be converted into acceptance by writing an exclusion. Labelled exclusions are permitted only for supplementary observations that are not one of the four gates, such as an unavailable additional platform run. Every supplementary exclusion must identify the unrun command or observation and the reason; it must not be relabelled green.

The repository hygiene command is a pre-commit guard, not a fifth test gate. Run and record:

- `cargo run -p a2a-bridge -- validate --repo-hygiene` before the implementation-candidate commit;
- `cargo run -p a2a-bridge -- validate --repo-hygiene` again before the handoff-only evidence commit.

A missing, incomplete, or nonzero hygiene guard forbids the corresponding commit. Record each invocation separately with its exit status; do not merge it into the four-gate table.

Also require:

- `git diff --check c637e493544a2e2edd1ca3ae20842a86dcb58f3f..<implementation-candidate-sha>` after the implementation candidate exists;
- `git diff --cached --check` against the final staged handoff immediately before the handoff-only commit;
- a source diff proving `crates/bridge-worktree/Cargo.toml` and `Cargo.lock` are unchanged from the base;
- the external integration-test crate to compile as part of the mandatory suites;
- the final counted-line worksheet to remain within every row cap and the total cap.

Bare `git diff --check` at a clean implementation commit is not evidence and must not be recorded as the candidate whitespace check.

### Interim A2a handoff and two-commit custody

Create:

`docs/superpowers/reviews/2026-08-19-r2f1b-3d-t3a-inc1-sliceA2a-handoff.md`

The complete inline schema in this section is the owner-approved in-container replacement for any external handoff template. The implementer must not search for, consult, or cite an external template. The repository contains no such template, and `AGENTS.md` requires none. The host-side operator separately applies the installed template to the operator’s own lane handoff; that host responsibility does not add an implementer dependency or another A2a artifact.

Use this two-commit protocol:

1. Implement and verify the source and test changes.
2. Run the repository hygiene guard and record its command and result for the implementation-candidate pre-commit point.
3. Commit the source and test changes as the implementation-candidate commit.
4. Re-run all four mandatory gates against that exact clean commit.
5. Run and record the base-to-candidate whitespace check and manifest/lockfile diff.
6. Author the interim handoff with the literal implementation-candidate SHA and complete evidence below.
7. Run the repository hygiene guard for the handoff-only pre-commit point and insert its command and result into the handoff.
8. Stage only the handoff.
9. Run `git diff --cached --check` against the final staged handoff immediately before committing.
10. Commit only the handoff as the handoff-only evidence commit.
11. Do not make the handoff self-name its own commit SHA. The final operator receipt may name that SHA after the commit exists.
12. A2b begins from the handoff-only commit, while the handoff binds behavioral evidence to the implementation-candidate commit and states that the second commit changes documentation only.

The handoff must contain:

- the exact base SHA and implementation-candidate SHA;
- the implementation commit subject and the handoff-only commit subject;
- clean-tree status before editing and at the implementation candidate;
- `rustc --version --verbose`, `cargo --version`, `rustfmt --version`, and `cargo clippy --version`;
- both pre-commit hygiene-guard commands exactly as run, their separate exit statuses, and confirmation that neither is classified as a test gate;
- every mandatory gate command exactly as run, its exit status, and its complete totals;
- separate counts for test binaries and doc-test suites;
- the exact base-to-candidate `git diff --check` command and result;
- the exact final `git diff --cached --check` command and result;
- the source diff proving the crate manifest and lockfile remained unchanged;
- the final actual counted-line worksheet, with every added nonblank line assigned exactly once;
- the evidence category and mutation caught by every new test;
- all labelled supplementary exclusions;
- a source-audit table with file-and-line evidence for these exact edges:
  - `sweep_orphans_with_exact_absence` →
    `sweep_orphans_with_exact_absence_with_pin_opener` →
    `checked_scan::scan_compatibility_with_pin_opener` →
    `scan_checked_rows_with_source`;
  - `scan_worktree_records` →
    `scan_worktree_records_with_pin_opener` →
    `checked_scan::scan_compatibility_with_pin_opener` →
    `scan_checked_rows_with_source`;
  - both production wrappers’ returned engine `Result` →
    their respective production result-to-projection functions;
  - injected `scan_checked_rows_for_test` results →
    the same two production result-to-projection functions;
- confirmation that the source/session traits, concrete compatibility source, concrete session, and iterator-entry refusal remain child-private;
- confirmation that every type named in a `pub(super)` signature or field is itself at least `pub(super)`;
- confirmation that the non-test production region has exactly one call site for each session-driving operation—`next_name`, `read_legacy`, `read_custody`, and `finish`—and that all four are inside `scan_checked_rows_with_source`;
- confirmation that `CheckedScanCompletedV1::into_rows` is called only by the action projection;
- confirmation that the exact projection retains canonical root, iterator-error count, root observations, checked rows, exact names, and production-computed decisions;
- confirmation that enumeration refusal retains the already-observed canonical root;
- confirmation that tests construct completed results only through `scan_checked_rows_for_test`;
- confirmation that the external integration test imports and pins both public functions;
- the exact `effective()` iterator item-type assertion;
- an event-source-audit row proving exactly one unguarded exact-absence event call site in the post-`finish` row loop, after decision-bearing row construction and driven from that stored decision, with no duplicate emitter;
- the unchanged event literal;
- the raw-action/canonical-exact root split;
- the A2b and future-attestation obligations from “Deferred”;
- the explicit statement that this inline schema is the owner-approved implementer-side replacement for any external template and that the host operator separately owns its lane-template application.

The source audit is PASS only if all listed edges, visibility facts, counts, and event conditions hold. A mismatch stops acceptance. Module privacy remains the enforcement mechanism preventing `sweep.rs` from opening or driving a checked-scan session.

A2b may replace this interim handoff with the final combined A2 handoff after completing report population, platform evidence, mutation audit, and its own gates.

### Deferred

Deferred to A2b:

- **F4-of-A2 — reproducible behavioral-red control.** A2a has no genuine runtime red because it preserves both public behaviors and the unit return. When A2b changes the return type, it must supply a frozen test-only patch against an exact recorded base tree, record that tree’s identity and patch diff, and run the genuine-red controls reproducibly before relying on them.
- **F6-of-A2 — source-incompatible public return change.** A2a makes no public break. A2b’s planned change from `()` to `ExactAbsenceSweepReportV1` remains source-incompatible for unit-constrained callers at workspace version `0.3.1`. A2b must resolve and record the release-version boundary as a blocking pre-publication obligation rather than relying only on handoff prose.
- **F8-of-A2 — birthtime-capability result visibility.** A2a adds neither filesystem attestation nor the birthtime-capability row. A2b must make the observed `Some` or `None` result visible through a captured probe or machine-readable artifact; a passing test whose evidence does not reveal the observed branch is insufficient.
- **F9-of-A2 — possible versus guaranteed resolver observations.** A2b’s mutation inventory must distinguish possible call edges from observations guaranteed on every comparator result. `compare_path_identities` installs `deepest_existing_path` as its resolver, but an unavailable initial resolution can return `CannotProve` before the final stability-bracket calls. The final calls must not be listed as unconditional observations.
- A2b owns the retained enumeration-descriptor implementation, root-observation classification and report population, the distinct 140-counted-line retained-enumerator row, scoped tracing evidence, UTF-8 characterization, complete platform matrix, final mutation audit, and final combined A2 handoff.
- A2b must consume the A2a rich exact outcome rather than re-canonicalizing after refusal, replacing the checked-scan seam, or recomputing A2a decisions.

Deferred to the separate attested real-filesystem slice sequenced with A2b’s platform matrix:

- **F3 — same-mount object replacement.** The future utility must prove how replacement of a fixture-root object on the same mount is detected rather than treating a stable mount label as stable object identity.
- **F7 — distro and environment labelling.** Future platform evidence must record a precise environment identity and must not infer a distribution or filesystem from an ambiguous runner label.
- **F12 — attestation record schema.** The future slice must define the complete durable machine-readable schema, required fields, versioning, refusal states, and completion binding before emitting platform claims.
- **F13 — synthetic coverage injection boundary.** The future utility must define the boundary that injects derived platform observations for deterministic refusal coverage without allowing injected values to green real-platform evidence.
- That slice owns real-filesystem fixture custody, independent derivation and comparison of platform observations, preflight-before-mutation proof, cleanup proof, same-root object-stability proof, and any dependency it separately justifies.

None of these deferred items may add scope, dependencies, worksheet lines, or acceptance rows to A2a.

### Sizing and mandatory pre-edit stop

Use this deterministic counted-line metric:

1. Measure the final two-commit A2a tree against base `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`.
2. For every owned changed file, count each added nonblank physical line after the fmt gate.
3. A replacement counts its added side. Deleted lines do not consume the cap.
4. No v3 seam line is exempt. The v2 exemption applied only to a byte-identical v2 seam; v3 deliberately restructures that seam.
5. Imports, attributes, declarations, macro invocations, assertions, comments, parameterized rows, test utilities, and nested constructs count by their nonblank added lines.
6. Assign every counted line to exactly one worksheet row by file and purpose.
7. If a line plausibly spans purposes, assign it to the first applicable row from top to bottom.
8. `git diff --numstat` may corroborate changed files but is not the final count because it includes blank lines.
9. The handoff file is counted like every other owned file.
10. `crates/bridge-worktree/Cargo.toml` and `Cargo.lock` have no row because they must not change.
11. There is no contingency row and no borrowing between rows.

Pre-edit worksheet:

| Counted component | Pre-edit estimate | Counted-line cap |
|---|---:|---:|
| `checked_scan.rs` complete v3 seam, compatibility source/session, completed result, classifier, engine, and production entry | 205 | 230 |
| `sweep.rs` action projection, rich exact outcome, refusal mapping, routing, decision retention, and event integration | 115 | 135 |
| `checked_scan.rs` injected source harness, operation log, ordering/status tests, and full deterministic production-projection matrix | 190 | 220 |
| `sweep.rs` routing, raw/canonical root, pin-failure, metadata-retention, decision-observation, and public-projection tests | 70 | 85 |
| External public-signature and exact `effective()` item-type assertions | 10 | 15 |
| Interim A2a handoff | 75 | 90 |
| **Total counted lines** | **665** | **775** |

The v3 estimate is 165 lines above v2’s 500-line estimate, and the v3 cap is 180 lines above v2’s 595-line cap. This increase is deliberate and required: the restructured seam no longer qualifies for v2’s literal-block exemption, the completed-result and row contracts are now fully pinned, the exact outcome retains metadata and decisions instead of erasing them, injected tests traverse production projections through an explicit factory, and the handoff adds event, whitespace, visibility, and two-point hygiene evidence. Do not compress declarations, conformance cases, or custody evidence to fit the obsolete v2 cap.

Before editing, re-estimate every row against the exact base. Stop if any row or the total will exceed its cap. Report the revised estimates and propose a narrower follow-up split; do not compress tests, declarations, evidence, or the handoff and do not silently extend A2a.

### Falsification license

Every symbol, caller, matrix row, visibility claim, and behavioral statement in this task is an anchored claim against `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`. The repository is authoritative.

If the base identity differs; the worktree is not clean; either public signature differs; `scan_worktree_records` does not use `read_dir` before pin opening; `read_sidecar` does not silently omit failures; `read_custody_record_in` does not retain the stated refusals; the exact and action routes do not enumerate canonical and raw root spellings respectively; the current exact route does not drain before assessment; the existing event differs; `ExactAbsenceRootRefusalV1` lacks either required refusal; the report surface, readiness gate, or constructor allowances differ; the dev-dependency set differs; `AGENTS.md` does not require the repository hygiene guard; a repository handoff-template requirement or file actually exists; bridge-core already exposes a cross-crate retained-descriptor enumerator suitable for this source; a listed production caller differs; the normative declarations cannot be made rustfmt-clean under the pinned toolchain; the checked-in CI lane does not run the stated workspace coverage command; or any injected conformance expectation is wrong, record the exact repository evidence and stop before editing.

Do not adapt the implementation around a false anchor. Finding the work smaller is acceptable. The A2a/A2b split, unit-return boundary for A2a, T3a-decides/T3b-acts boundary, no-new-dependency rule, exact seam visibility decision, and T3b action-time re-decision remain settled.

## Acceptance Criteria

1. Work begins only from exact clean base `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`, after factual and sizing checks.
2. `crates/bridge-worktree/src/sweep/checked_scan.rs` exists and contains the complete v3 seam with the pinned names, fields, types, and visibility.
3. Every normative Rust declaration passes rustfmt under the pinned Rust 1.94.0 toolchain.
4. No module-wide, type-wide, `impl`-wide, or function-wide `dead_code` allowance is added.
5. Within `checked_scan.rs`, only the six pinned field-scoped allowances are present; `report.rs`’s four existing constructor allowances remain additional and unchanged.
6. Every type named in a `pub(super)` signature or field is itself at least `pub(super)`.
7. `CheckedScanOpenRefusalV1`, `CompatibilityPinOpenerV1`, `FilesystemCompatibilityPinOpenerV1`, `RootIdentityCaptureV1`, `RootObservationSetV1`, `CheckedScanRowV1`, `CheckedScanCompletedV1`, and `scan_compatibility_with_pin_opener` have the pinned `pub(super)` visibility.
8. The source/session traits, iterator-entry refusal, concrete compatibility source, and concrete session remain private to `checked_scan.rs`.
9. `sweep.rs` receives completed scan results and row/root vocabulary but cannot open or drive a checked-scan session.
10. The compatibility source calls `read_dir` before the custody pin opener and refuses source open only when `read_dir` fails.
11. Pin failure preserves legacy reads and retains custody names with the exact not-pinnable refusal.
12. Production `finish` returns `RootObservationSetV1::default()`.
13. A2a does not classify or populate any production root observation.
14. One shared classifier preserves the base legacy and custody selection rules.
15. `scan_checked_rows_with_source` has the exact pinned signature and exclusively owns the production `next_name` → immediate selected read → `finish` protocol.
16. The engine returns an owned completed result and passes only `CannotEnumerate` from source open.
17. `iterator_error_count` is a `usize`; iterator-item errors are counted and skipped.
18. Custody refusals remain rows, while malformed or unreadable legacy sidecars remain omitted.
19. Exact `OsString` names survive engine enumeration without reconstruction from lossy display text.
20. In non-test code, only the engine constructs checked rows and completed results.
21. Injected tests construct completed results only through `scan_checked_rows_for_test`, which delegates to the real engine without duplicated logic.
22. `CheckedScanCompletedV1::into_rows` has the exact pinned consuming signature and is used only by the action projection.
23. `project_action_scan_result` and `project_exact_scan_result` have the exact pinned signatures and are used by the filesystem wrappers and injected matrix.
24. `scan_worktree_records_with_pin_opener` has the exact pinned signature and delegates engine refusal and result projection to `project_action_scan_result`.
25. `sweep_orphans_with_exact_absence_with_pin_opener` has the exact pinned signature and delegates successful-canonicalization engine results to `project_exact_scan_result`.
26. Exact-route canonicalization completes before opener consultation.
27. Canonicalization refusal uses `ExactAbsenceRootRefusalV1::CannotCanonicalize` and retains no canonical root.
28. Enumeration refusal uses `ExactAbsenceRootRefusalV1::CannotEnumerate` and retains the canonical root observed before source open.
29. No duplicate exact-scan refusal vocabulary is introduced.
30. Only the action projection erases exact names, iterator status, and root observations.
31. The rich exact outcome retains canonical root, `usize` iterator-error count, complete root observations, checked rows, exact names, selected records, and production-computed decisions.
32. Exact assessment begins only after complete enumeration, selected reads, terminal `None`, and exactly one `finish`.
33. Every exact decision is computed once, stored in `ExactScanProjectionRowV1`, and exposed to private tests.
34. The unchanged event uses the decision stored in that same row rather than an independently computed value.
35. The non-test source contains exactly one unguarded exact-absence event call site, inside the post-`finish` row loop and after decision-bearing row construction.
36. The deterministic conformance matrix invokes both production result projections and contains no duplicated projection or decision logic.
37. The matrix compares full legacy values, full decoded custody values, exact custody refusals, omission, injected ordering, iterator continuation, pin-failure policy, exact names, retained metadata, and production-computed decisions.
38. Injected non-default root observations leave action rows and exact decisions unchanged while surviving in the rich exact outcome.
39. Injected source-open refusal produces an empty action projection, a canonical-root-bearing exact refusal, and zero assessments.
40. No ordered-equality oracle compares independent real `read_dir` traversals.
41. `scan_worktree_records` retains its public visibility, exact signature, eager vector projection, raw-root behavior, flattened iterator errors, and existing callers.
42. `sweep_orphans_with_exact_absence` retains its public visibility, exact unit-returning signature, canonical scan root, existing decisions, exactly-one-per-row event behavior, and unchanged event literal.
43. The external integration test compiler-enforces both public scan declarations.
44. The external compile-time assertion pins `effective()` to `Iterator<Item = &ExactAbsenceSweepEntryV1>`.
45. `sweep_orphans` retains its statement-position exact call, independent guard, warning, early return, canonical `root_cwd` decisions, and raw action-scan argument.
46. No public report type, constructor allowance, readiness rule, API documentation, or production report code changes in A2a.
47. No tracing capture, public reporter, global subscriber, or new dependency is added.
48. `crates/bridge-worktree/Cargo.toml` remains unchanged with exactly its two existing dev-dependencies.
49. `Cargo.lock` remains unchanged.
50. No filesystem-attestation utility, platform fixture gate, real-traversal ordering oracle, or platform-labelled A2a acceptance row is added.
51. No test is labelled genuine runtime red; every new test names its evidence category and production or evidence-infrastructure mutation.
52. The retained-enumeration-object meaning remains identity from the exact enumeration descriptor, and A2a does not fabricate it from `ReadDir`, a path, or the custody pin.
53. A2b’s retained-descriptor obligation and distinct 140-line budget remain recorded.
54. The inline handoff schema is explicitly identified as the owner-approved implementer-side replacement for any external template, with separate host-operator responsibility recorded.
55. The interim handoff binds the exact implementation-candidate commit, commands, toolchain, outcomes, source audit, exclusions, whitespace evidence, pre-commit guards, and final worksheet.
56. The repository hygiene guard exits zero before each of the two commits, and both results are recorded separately from the test gates.
57. The base-to-candidate `git diff --check` and final staged `git diff --cached --check` both exit zero and are recorded.
58. The final A2a tip consists of the implementation-candidate commit followed by a handoff-only evidence commit that does not self-name.
59. Fmt, clippy, the full locked workspace suite, and the locked `bridge-worktree` suite all exit zero; no exclusion substitutes for a mandatory gate.
60. Test totals are reported as test binaries plus doc-test suites without nested-output double counting.
61. Every counted worksheet row and the 775-line total remains within cap.
62. No ownership, locking, transition, publication, settlement, deletion, prune, rename, backend-cleanup, or T3b authority is introduced.
63. The resulting two-commit A2a tip is suitable as A2b’s base without changing either public scan signature or replacing the checked-scan seam.

## Files

- `crates/bridge-worktree/src/sweep.rs`
  - declare the private checked-scan child module without a module-wide allowance;
  - import only the pinned `pub(super)` seam vocabulary;
  - add the private decision-bearing exact row, complete exact result, and rich exact outcome;
  - implement both production result-to-projection functions with their pinned signatures;
  - implement `scan_worktree_records_with_pin_opener` with its pinned signature;
  - delegate the public action scanner through the production action projection;
  - implement `sweep_orphans_with_exact_absence_with_pin_opener` with its pinned signature and canonicalization mapping;
  - delegate the public unit-returning exact route through the rich private outcome;
  - preserve all exact decisions, event fields/message/count/order, action behavior, guard behavior, removal behavior, and run-end behavior;
  - add routing, raw/canonical-root, pin-failure, metadata-retention, decision-observation, and public-projection tests.
- `crates/bridge-worktree/src/sweep/checked_scan.rs`
  - create with the complete v3 seam;
  - implement the private `ReadDir`-backed compatibility source and session;
  - implement the shared classifier, checked row, completed result, action-only `into_rows`, and mandatory engine;
  - preserve independent custody pin failure;
  - return default production root observations;
  - provide the private test-only engine factory;
  - add the child-private injected source/session harness, operation log, full deterministic production-projection matrix, ordering/status tests, exact-name test, and non-default-observation evidence.
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs`
  - import and compile-assert both public scan functions with their complete signatures;
  - replace the untyped `effective()` call check with the exact iterator item-type assertion;
  - leave production report code unchanged.
- `docs/superpowers/reviews/2026-08-19-r2f1b-3d-t3a-inc1-sliceA2a-handoff.md`
  - create after the implementation-candidate commit;
  - use the complete owner-approved inline schema in this specification;
  - record exact commit identity, source audit, toolchain, mandatory gates, totals, classifications, exclusions, whitespace checks, both pre-commit hygiene guards, and final worksheet;
  - commit as the sole change in the handoff-only evidence commit;
  - do not self-name the handoff commit.
- `AGENTS.md`
  - read-only repository-contract reference for the pre-commit hygiene guard and absence of an in-repository external-template requirement;
  - do not modify.
- `crates/bridge-worktree/Cargo.toml`
  - read-only dependency reference;
  - do not modify.
- `Cargo.lock`
  - read-only locked-resolution reference;
  - do not modify.
- `crates/bridge-worktree/src/sweep/report.rs`
  - read-only reference for the A1 report surface, existing refusal vocabulary, and four existing constructor allowances;
  - do not modify in A2a.
- `crates/bridge-worktree/src/provider_path.rs`
  - read-only production reference for `canonicalize_lenient`, `read_sidecar`, and `WorktreeSidecar`;
  - do not modify.
- `crates/bridge-worktree/src/custody.rs`
  - read-only production reference for name selection, custody decoding, and custody refusals;
  - do not modify.
- `crates/bridge-worktree/src/custody_lock.rs`
  - read-only caller and regression reference for the public action scanner;
  - do not modify.
- `crates/bridge-core/src/fs_custody.rs`
  - read-only reference for `BirthTimeV1`, `PinnedDirectoryV1`, descriptor reads, the crate-private enumerator, and the deferred comparator inventory;
  - do not modify in A2a.
- `crates/bridge-core/src/session_cwd.rs`
  - read-only reference for the canonical-root type retained by the rich exact outcome;
  - do not modify.
- `Cargo.toml`
  - read-only workspace version and dependency reference;
  - do not modify.
- `rust-toolchain.toml`
  - read-only pinned-toolchain reference;
  - do not modify.
- `.github/workflows/ci.yml`
  - read-only reference for the existing Ubuntu workspace coverage lane and repository hygiene job;
  - do not modify.
- `bin/a2a-bridge/src/main.rs`
  - read-only caller-audit reference;
  - no CLI changes.

## Spec Refs

Authoritative at base commit `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`:

- `AGENTS.md`
- `Cargo.toml`
- `Cargo.lock`
- `rust-toolchain.toml`
- `.github/workflows/ci.yml`
- `crates/bridge-worktree/Cargo.toml`
- `crates/bridge-worktree/src/sweep.rs`
- `crates/bridge-worktree/src/sweep/report.rs`
- `crates/bridge-worktree/src/provider_path.rs`
- `crates/bridge-worktree/src/custody.rs`
- `crates/bridge-worktree/src/custody_lock.rs`
- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-core/src/session_cwd.rs`
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs`
- `bin/a2a-bridge/src/main.rs`
- `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-inc1-sliceA1-handoff.md`

## Commit Message

Implementation-candidate commit:

```text
refactor(worktree): unify exact and action scans

Add one private production-bound checked-scan engine and route both the
unit-returning exact-absence path and the existing action scanner through it.

Preserve raw action-root behavior, canonical exact-root behavior, legacy
omission, custody refusals, eager ordering, and public return types while adding
deterministic injected-source conformance and exact API assertions for A2b.
```

Handoff-only evidence commit:

```text
docs(worktree): record A2a scan evidence

Bind the accepted scan-engine candidate to its source audit, pinned toolchain,
mandatory gate totals, evidence classifications, exclusions, pre-commit guards,
whitespace checks, and final counted-line worksheet.
```
