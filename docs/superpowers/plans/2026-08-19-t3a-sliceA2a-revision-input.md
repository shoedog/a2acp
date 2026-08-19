---
task-type: implement
---

# Revise the A2a task spec to v2 — lift attestation out, fold eleven findings

## Description

Your A2a spec v1 drew 17 review findings. Eleven are closed and enumerable and
you should fold them. Six are instances of one mechanism — the attested
fixture-root utility — and the owner has **removed that mechanism from A2a**
rather than have you specify it under this cap.

Produce the **complete revised A2a task spec v2**. Not a review response, not a
changelog, not a diff — the whole spec, ready for an implementer.

Emit it between the extraction markers, nothing outside them.

### Scope change — read this before folding anything

**Real-filesystem attested conformance is REMOVED from A2a.** Delete the
`### Attested fixture-root mechanism` section entirely, along with its
environment variables, JSON attestation records, `statfs`/`findmnt` derivation,
preflight and real-fixture ignored tests, synthetic attestation coverage, and
the `libc` dev-dependency. Remove APFS/ext4 rows from A2a's acceptance criteria.

**A2a proves projection equivalence with injected deterministic sources
instead.** This is not a weaker substitute — for A2a's actual claim it is
stronger evidence, because an injected source can force pin failure, malformed
legacy sidecars, unreadable custody records, iterator errors, and exact
enumeration orderings on demand, none of which a real directory reliably
produces. Design the injected-source evidence to carry the full conformance
matrix.

Why this is defensible, so you can state it in the spec rather than merely
assert it:

- A2a adds no new filesystem observation. It preserves `read_dir`,
  `read_sidecar`, `read_custody_record_in`, and pin-open semantics exactly, and
  its own contract forbids behavior change and forbids genuine runtime red.
- CI already runs `cargo llvm-cov --workspace` on `ubuntu-latest`, which is an
  existing ext4 exercise of the `bridge-worktree` suite, and the operator's host
  gate runs the same suite on macOS/APFS. A2a therefore still gets both
  filesystems — it simply stops asserting a bespoke attested claim about them.

Record this in a short `### Filesystem evidence posture` section: state what
A2a does and does not claim about filesystem behavior, and state that attested
real-filesystem conformance is a separate slice sequenced with A2b's platform
matrix, where the birthtime-capability question already lives.

**F6 dissolves with it.** The reviewer correctly noted that two independent
`read_dir` traversals have unspecified relative order, so an ordered-equality
oracle across real traversals can fail spuriously. With injected deterministic
name streams that problem does not arise. Prove order preservation on the
injected stream; do not build a real-filesystem ordering oracle.

### Environment facts

Your working tree is at `c637e493`, the base commit, and the repository is
authoritative. Read the code. Where you assert a fact, you may be asked which
file and line you read it from.

You cannot read anything outside the repository — no `~`, no `$HOME`, no
installed templates — and the spec must never name a path outside the
repository, because the implementer runs in a container with only the code tree
mounted.

---

## Operator-verified findings — measured, not open to reinterpretation

### F1 (BLOCKER) — the lockfile dead end is real, but it dissolves with attestation

Measured in a scratch worktree at `c637e493`, with a before/after control:

| State | `cargo metadata --locked` |
|---|---:|
| base | exit 0 |
| + `libc` in `bridge-worktree` dev-deps | **exit 101**, `cannot update the lock file ... because --locked was passed` |
| restored | exit 0 |

The delta was exactly one line — `"libc",` added to the `bridge-worktree`
dependencies list in `Cargo.lock` — with no version-resolution change, because
`libc 0.2` is already locked for `bridge-acp`, `bridge-core`, `bridge-store`,
and the binary.

**Resolution:** since attestation is removed, A2a adds no dev-dependency at all.
`crates/bridge-worktree/Cargo.toml` returns to read-only, its dev-dependency set
stays exactly `bridge-coordinator` and `bridge-controller`, and `Cargo.lock` is
untouched and needs no worksheet row. If any part of your v2 still requires a
new dependency, that is a signal you have not fully removed the mechanism.

### F5 (BLOCKER) — A2a needs its own durable handoff

v1 deferred all handoff work to A2b while keeping conformance evidence only in
`--nocapture` output. A2a is intended to end at an accepted, stable, committed
point, and this repository requires a handoff at such a point. The source audit,
gate totals, exclusions, and sizing worksheet would otherwise have no durable
custody.

Add an interim A2a handoff bound to the exact commit, commands, toolchain,
outcomes, and exclusions, and give it a worksheet row. Keep v2's two-commit
custody protocol (implementation-candidate commit, then a handoff-only evidence
commit that does not self-name its own SHA). A2b may still own the final
combined A2 handoff; say so.

Write the handoff requirements inline. Do not instruct the implementer to
consult a template — it cannot read one.

---

## Closed findings to fold

### F4 (BLOCKER) — contradictory completion rule

AC25 required both attested platform rows; AC31 permitted "exact totals **or
explicit exclusions**." The same evidence both blocked and permitted acceptance.
The attested rows are gone, but the contradiction pattern must not survive:
state **one** completion rule for A2a's gates, and make explicit which results
are mandatory for acceptance versus which may be reported as labeled exclusions.

### F8 (SMELL) — unpinned signatures

`scan_checked_rows_with_source` and
`sweep_orphans_with_exact_absence_with_pin_opener` have no exact return
signatures or refusal mappings. `Option`, `Result`, a sentinel, or a test-only
exposure would each yield materially different implementations and tests.

Pin both complete signatures: engine-result ownership, canonicalization
refusal, source-open refusal, assessment timing, and the exact mechanism by
which module tests inspect the private result.

### F9 (WRONG) — the discard claim is untestable at runtime

Both projection helpers accept only a pin opener, and the production
compatibility session always returns default root observations. A runtime test
therefore cannot distinguish correct discarding of non-default observations from
incorrect handling of them.

Either classify this as an exact source/type audit with pinned assertions, or
add a test-only projection seam that supplies non-default observations without
altering the production opener contract. Choose one and say which.

### F10 (SMELL) — visibility is not proven

In-module compiler checks constrain return types but cannot prove the two
functions remain `pub`; all current callers are internal, so an accidental
visibility reduction still compiles. Authorize an external compile-time API
assertion or a deterministic source guard pinning visibility and complete
signatures.

### F11 (SMELL) — "record a symbol-scoped source audit" has no mechanism

AC27 leans on that audit to prove a universal negative (no second production
session-driving loop), but the audit has no defined mechanism, durable
location, or pass/fail contract. Meanwhile exposing all session-driving methods
as `pub(super)` leaves exclusivity dependent on it.

The reviewer's structural suggestion is worth taking seriously: put the engine
beside the private source and session in `checked_scan.rs` and expose only the
completed result to `sweep.rs`, so **module privacy enforces exclusivity** and
the audit stops being load-bearing. If you keep the engine in the parent, you
must specify a durable guard with exact symbols, permitted edges, forbidden
calls, expected counts, and failure behavior. Pick one and justify it in one
sentence.

### F14 (SMELL) — the byte-pinned block carries A2b's unused declarations

v1 lands the entire pinned seam — including `RootIdentityCaptureV1`,
`RootObservationSetV1`, `complete_identity`, and `classify_root_observations`,
which A2a never consumes — behind a module-wide `dead_code` allowance that can
also conceal genuinely accidental dead code.

This is a real tension and you should resolve it explicitly rather than repeat
v1's justification. The byte-identity requirement exists so A2a and A2b share
one seam without a reformatting round; a module-wide allowance that hides
unrelated dead code is a real cost. If you keep the whole block, narrow the
allowance to the specific unused items rather than the module. If you land only
the consumed subset, the subset must still pass `rustfmt --check` as written and
you must say what A2b adds.

**Constraint that does not move:** any seam text A2a lands must be
rustfmt-clean under the pinned toolchain. The operator verifies this
mechanically — `rustfmt --check --edition 2021` exits 0 on v1's block today, and
round 2 of the A2 review raised a rustfmt BLOCKER against that same block that
was **refuted** by measurement. Do not reformat pinned text in response to a
formatting concern; only a narrower or wider selection of it is on the table.

### F16 (MINOR) — evidence-infrastructure tests can't document a production mutation

"Every new test must document the production mutation it catches" conflicts with
tests categorized as evidence infrastructure. Require each test to document the
"production or evidence-infrastructure mutation" it catches and name its
category.

### F17 (SMELL) — the frozen external check doesn't pin the item type

`let _ = report.effective();` does not prove
`Iterator<Item = &ExactAbsenceSweepEntryV1>`. Permit a test-only compile-time
assertion of the exact item type, leaving production report code unchanged.

### F15 (SMELL) — re-estimate

Re-derive every worksheet row after the scope change. Removing attestation
should reduce the total substantially; adding the interim handoff adds a row.
Keep v1's deterministic counted-line metric — added nonblank physical lines
after the fmt gate, one row per line, no contingency, no borrowing. Report an
honest estimate against a cap you set. If the estimate lands well under v1's
760, say so plainly rather than padding to fill it.

### F3, F7, F12, F13 — REMOVED WITH THE MECHANISM

Same-mount object replacement, distro labelling, attestation JSON schema, and
the synthetic-coverage injection boundary all belonged to the attested fixture
utility. Do not carry them forward. Record in `### Deferred` that they belong to
the future attestation slice, so that slice starts with them already known.

---

## Deferred to A2b — carry forward unchanged

Keep v1's `### Deferred to A2b` block, which correctly records F4-of-A2
(reproducible behavioral-red control), F6-of-A2 (the source-incompatible public
return change at `0.3.1`), F8-of-A2 (birthtime-capability result visibility),
and F9-of-A2 (possible versus guaranteed resolver observations). Add the
attestation slice as a new deferred item.

## Output contract

Emit the complete A2a task spec v2 between the markers, with:

- the same front matter (`task-type: implement`);
- `## Description`, `## Acceptance Criteria`, `## Files`, `## Spec Refs`,
  `## Commit Message`;
- `### Filesystem evidence posture` stating what A2a does and does not claim;
- no attested-fixture mechanism, no new dependency, no `Cargo.lock` change;
- every closed finding above folded, each resolved to one answer, not a menu;
- `### Deferred` covering both A2b's items and the attestation slice's;
- a falsification license: the repository is authoritative, and an implementer
  who finds a stated anchor false must stop and report rather than adapt;
- no path outside the repository anywhere in the document.

---

## Reference — A2a spec v1, verbatim

Revise the document below. Reproduce heading levels as they appear. This
reference ends at the end of the document.


# R2f1b 3d T3a increment 1, slice A2a — production-bound compatibility scan engine

## Description

Implement slice A2a against exact base commit `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`.

Direct read-only inspection of the clean repository tree at that commit confirmed:

- `sweep_orphans_with_exact_absence` returns `()`, canonicalizes the supplied root with `canonicalize_lenient`, drains `scan_worktree_records`, makes the existing decisions, and emits the existing per-row event;
- `scan_worktree_records` eagerly returns `Vec<(String, ScannedWorktreeRecordV1)>`;
- that scanner calls `std::fs::read_dir` before attempting `PinnedDirectoryV1::open`;
- a source-open failure returns an empty vector;
- iterator-item errors are flattened;
- malformed or unreadable legacy sidecars are silently omitted through `read_sidecar`;
- custody-named entries remain represented as decoded custody records or `UnreadableCustody`;
- custody pin failure preserves legacy reads and yields
  `CustodyReadRefusalV1::Unreadable("sweep root is not pinnable".to_string())` for custody names;
- the exact-absence route passes its lenient-canonical root to the scanner;
- `sweep_orphans`, `WorktreeRunEndGuard`, and the custody-lock test use the public action scanner with the raw root supplied by their caller;
- `sweep_orphans` calls `sweep_orphans_with_exact_absence` in statement position, then independently canonicalizes its action guard root;
- `EXACT_ABSENCE_POLICY_READY_V1` remains false;
- the A1 report module remains 598 lines and retains four constructor `dead_code` allowances;
- `crates/bridge-worktree/Cargo.toml` has exactly two dev-dependencies: `bridge-coordinator` and `bridge-controller`;
- the workspace already declares `libc`;
- the pinned toolchain is Rust 1.94.0 with rustfmt and clippy;
- `PinnedDirectoryV1` exposes its captured identity and descriptor-relative custody reads, while bridge-core’s existing descriptor-name enumerator is crate-private and therefore unavailable to `bridge-worktree`;
- `deepest_existing_path` is defined at `crates/bridge-core/src/fs_custody.rs:1511` and installed as the production resolver at line 1777.

These are source-tree anchors, not build or test evidence. No build or test was run while authoring this specification.

Before editing, verify the exact base commit and re-read every authoritative file under “Spec Refs.” If any factual anchor is false at that commit, apply the falsification license instead of adapting the implementation to a stale claim.

### Scope and settled boundary

A2a owns:

- the private checked-scan module and real compatibility source;
- one production-bound checked-scan engine used by the exact-absence and compatibility/action projections;
- one shared display-name selection and read policy;
- a private exact-route pin-opener seam below supplied-root canonicalization;
- deterministic equivalence evidence for both projections, including pin failure and decoded custody records;
- preservation of the public action scanner’s eager vector projection and raw-root behavior;
- preservation of the public exact-absence function’s unit return and current assessment and event behavior;
- the decision about enumeration-descriptor ownership on the A2a/A2b boundary;
- filesystem-fixture supply and attestation infrastructure for same-root conformance evidence;
- APFS and ext4 same-root conformance runs using that attested infrastructure;
- structural, mechanism, and characterization evidence for the refactor.

A2a does not:

- change the public signature or return type of `sweep_orphans_with_exact_absence`;
- populate an `ExactAbsenceSweepReportV1`;
- change any public report type, accessor, iterator, readiness rule, or constructor allowance;
- add report refusal, requested-root, canonical-root, iterator-status, entry, root-observation, or effective-entry behavior;
- add eager report assessment or new tracing behavior;
- add scoped tracing capture or a `tracing-subscriber` dependency;
- add birthtime-capability evidence;
- populate authoritative root captures in production;
- characterize registration-path UTF-8 behavior;
- perform the final mutation audit or final A2 handoff;
- add the increment-2 population-admission rule;
- set `EXACT_ABSENCE_POLICY_READY_V1` to true;
- construct `IneligiblePopulation` or `CannotConstructSubject` production assessments;
- add ownership, locking, transition, publication, settlement, unlink, removal, prune, rename, backend-cleanup, or T3b authority;
- change CLI behavior;
- claim that a scan result is action authority.

A2b starts from the accepted A2a commit and owns report return and population, eager assessment and tracing evidence, root and birthtime-capability evidence, UTF-8 characterization, the complete platform matrix, the mutation audit, and the final handoff.

T3a decides and reports. T3b will independently re-open, re-read, re-bind, re-apply admission, re-prove exact absence, and retain its own lock and action-time authority through any later effect.

### Public API preservation

The public exact-absence declaration remains exactly:

```rust
pub fn sweep_orphans_with_exact_absence(root: &str, probe: &dyn ExactAbsenceProbeV1)
```

It continues to return `()`. Do not add a report-returning overload, compatibility wrapper, new public scanner, or public test seam.

The public action scanner remains:

```rust
pub fn scan_worktree_records(root: &str) -> Vec<(String, ScannedWorktreeRecordV1)>
```

Every existing caller continues to compile without source changes unless an import or internal routing change is mechanically necessary. The fifteen report types, `effective()` iterator, false readiness gate, four A1 constructor allowances, and stale A1-to-A2 constructor comments remain untouched for A2b.

Because A2a adds no public break and no new observable behavior, it has no genuine behavioral-red test against the untouched base. Its evidence is base-green characterization, new-private-seam mechanism evidence, refactor conformance, and source structure. Do not manufacture a runtime-red claim.

### Cross-module seam and A2a/A2b declaration division

Create `crates/bridge-worktree/src/sweep/checked_scan.rs` and declare it as a private child module of `sweep.rs`.

A2a lands the entire pinned seam block below. A smaller excerpt would either leave `CheckedScanRootSessionV1::finish` without its declared result type or require rewriting the pinned import and signature lines. The whole block therefore lands byte-for-byte, without reformatting:

```rust
use std::ffi::{OsStr, OsString};
use std::path::Path;

use bridge_core::fs_custody::{BirthTimeV1, PinnedDirectoryV1};

use crate::custody::{CustodyReadRefusalV1, WorktreeCustodyRecordV1};
use crate::provider_path::WorktreeSidecar;

use super::report::CustodyRootObservationV1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CheckedScanOpenRefusalV1 {
    CannotEnumerate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CheckedScanEntryRefusalV1 {
    CannotReadEntry,
}

pub(super) trait CheckedScanSourceV1 {
    fn open(
        &self,
        enumeration_root: &Path,
    ) -> Result<Box<dyn CheckedScanRootSessionV1>, CheckedScanOpenRefusalV1>;
}

pub(super) trait CheckedScanRootSessionV1 {
    fn next_name(&mut self) -> Option<Result<OsString, CheckedScanEntryRefusalV1>>;

    fn read_legacy(&self, enumerated_name: &OsStr, record_display: &str)
        -> Option<WorktreeSidecar>;

    fn read_custody(
        &self,
        enumerated_name: &OsStr,
    ) -> Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1>;

    fn finish(self: Box<Self>) -> RootObservationSetV1;
}

/// Test injection is below the real compatibility source. Tests replace only the
/// pin-open result while retaining production `read_dir`, name selection, legacy
/// reading, custody handling, session state, and finish behavior.
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

pub(super) struct CompatibilityCheckedScanSourceV1<P> {
    pin_opener: P,
}

impl<P: CompatibilityPinOpenerV1> CompatibilityCheckedScanSourceV1<P> {
    pub(super) const fn new(pin_opener: P) -> Self {
        Self { pin_opener }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RootIdentityCaptureV1 {
    dev: Option<u64>,
    ino: Option<u64>,
    birthtime: Option<BirthTimeV1>,
}

impl RootIdentityCaptureV1 {
    fn complete_identity(&self) -> Option<(u64, u64, BirthTimeV1)> {
        Some((self.dev?, self.ino?, self.birthtime?))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RootObservationSetV1 {
    retained_enumeration_object: Option<RootIdentityCaptureV1>,
    pinned_custody_directory: Option<RootIdentityCaptureV1>,
    final_named_root: Option<RootIdentityCaptureV1>,
}

pub(super) fn classify_root_observations(
    observations: &RootObservationSetV1,
) -> CustodyRootObservationV1 {
    let (Some(retained_enumeration_object), Some(pinned_custody_directory), Some(final_named_root)) = (
        observations
            .retained_enumeration_object
            .and_then(|identity| identity.complete_identity()),
        observations
            .pinned_custody_directory
            .and_then(|identity| identity.complete_identity()),
        observations
            .final_named_root
            .and_then(|identity| identity.complete_identity()),
    ) else {
        return CustodyRootObservationV1::Unavailable;
    };

    if retained_enumeration_object == pinned_custody_directory
        && pinned_custody_directory == final_named_root
    {
        CustodyRootObservationV1::Pinned
    } else {
        CustodyRootObservationV1::IdentityChanged
    }
}
```

The corresponding module declaration may carry `#[allow(dead_code)]` because A2a deliberately lands A2b’s root-observation declarations without consuming `classify_root_observations` in production. That allowance applies only to the private module and is explicitly A2b-pending. Do not alter the pinned block to place allowances inside it.

Declaration ownership is explicit:

- A2a consumes `CheckedScanOpenRefusalV1`, `CheckedScanEntryRefusalV1`, `CheckedScanSourceV1`, `CheckedScanRootSessionV1`, `CompatibilityPinOpenerV1`, `FilesystemCompatibilityPinOpenerV1`, and `CompatibilityCheckedScanSourceV1<P>`.
- A2a carries `RootIdentityCaptureV1`, `RootObservationSetV1`, `complete_identity`, and `classify_root_observations` only because the pinned session signature and seam block are indivisible without rewriting.
- A2a’s production `finish` returns `RootObservationSetV1::default()`.
- A2a does not call `classify_root_observations`, expose root evidence, or test root-classification capability.
- A2b owns real population and use of the three root observations, removal of the temporary module-level allowance, classifier and capability evidence, and report projection.

The concrete compatibility session remains private to `checked_scan.rs`. Parent `sweep.rs` sees only the declared seam, not its stored `ReadDir`, optional custody pin, or other session internals.

### Compatibility source

Implement `CompatibilityCheckedScanSourceV1<P>` with the current filesystem policy and parameterize only custody pin opening.

Its A2a open sequence is exact:

1. Call `std::fs::read_dir(enumeration_root)`.
2. Return `CheckedScanOpenRefusalV1::CannotEnumerate` only if that call fails.
3. After successful `read_dir`, call `open_pin(enumeration_root)`.
4. Retain the `ReadDir` and returned `Option<PinnedDirectoryV1>` in the private session.
5. Permit legacy reads through path-based `read_sidecar` regardless of pin outcome.
6. With no custody pin, return
   `CustodyReadRefusalV1::Unreadable("sweep root is not pinnable".to_string())`
   for every custody read.
7. Consume the session in `finish` and return `RootObservationSetV1::default()`.

A pin failure is not a source-open failure. A readable directory with a failed custody pin must still enumerate completely, preserve valid legacy rows, and retain every custody-named row with the exact not-pinnable refusal.

`next_name` maps each successful `ReadDir` item to its exact `OsString`, maps each iterator error to `CheckedScanEntryRefusalV1::CannotReadEntry`, and returns `None` only when enumeration ends. It must not reconstruct an exact name from lossy display text.

`read_legacy` calls the existing `read_sidecar(record_display)`. `read_custody` calls the existing `read_custody_record_in` with the retained custody pin and exact enumerated name. No new legacy decoder, custody decoder, selection vocabulary, or read policy is permitted.

### Shared selection and mandatory scan engine

Extract one private display-path classifier in `sweep.rs` for:

- legacy `*.meta.json`;
- custody names selected by `is_custody_record_name`;
- ignored names.

The classifier receives the lossy full display path formed by joining the supplied enumeration root with the exact enumerated name. It preserves the current legacy suffix rule and custody selector exactly.

Create private intermediate types in `sweep.rs`:

- one row containing the lossy display path, exact `OsString`, and `ScannedWorktreeRecordV1`;
- one engine result containing ordered rows, iterator-error count, and `RootObservationSetV1`.

Create one private parent-module engine named `scan_checked_rows_with_source`. It is the only production code permitted to drive `CheckedScanRootSessionV1`.

Its protocol is exact:

1. Call `source.open(enumeration_root)`.
2. Repeatedly call `next_name`.
3. Count an `Err` item and continue.
4. For each successful name, retain the exact `OsString`.
5. Construct the same lossy full display path the base scanner obtains from `DirEntry::path`.
6. Invoke the one shared display classifier.
7. Immediately perform the selected legacy or custody read before requesting the next name.
8. Silently omit a legacy row when `read_sidecar` returns `None`.
9. Retain decoded custody and custody-refusal rows in enumeration order.
10. Ignore unrelated names.
11. Call `finish` exactly once after `next_name` returns `None`.
12. Return the ordered rows, iterator-error count, and observation set.

A custody read or decode refusal is a retained row, not an iterator error. A malformed or unreadable legacy sidecar remains silently omitted and contributes neither a row nor an iterator error.

No other production loop may call `next_name`, `read_legacy`, `read_custody`, or `finish`.

### Production projections

Add a private `scan_worktree_records_with_pin_opener` helper. It:

- accepts the caller’s raw `root` and only a `CompatibilityPinOpenerV1`;
- constructs `CompatibilityCheckedScanSourceV1`;
- invokes `scan_checked_rows_with_source` with that raw root;
- returns an empty vector on source-open refusal;
- discards iterator-error and root-observation details;
- projects each retained row to the existing `(String, ScannedWorktreeRecordV1)` shape.

Production `scan_worktree_records` supplies `FilesystemCompatibilityPinOpenerV1`.

The public action scanner retains every existing observable:

- the public signature and eager vector return;
- raw-root `read_dir` and raw-root pin opening;
- no canonicalization of its enumeration argument;
- source-open failure producing an empty vector;
- iterator-item errors being flattened;
- legacy reads using the lossy full display path;
- malformed or unreadable legacy sidecars being omitted;
- custody selection using `is_custody_record_name`;
- custody reads using the exact enumerated `OsString`;
- decoded custody records retaining full structural equality;
- custody refusals retaining their exact variants and messages;
- pin failure preserving legacy rows and refusing custody rows as not pinnable;
- enumeration order.

Existing consumers of `scan_worktree_records`, including the run-end guard and custody-lock coverage, continue to use only this public projection.

### Report-side pin-opener seam and projection equivalence

Add one private report-side helper named
`sweep_orphans_with_exact_absence_with_pin_opener`.

It accepts the raw supplied root, the existing probe, and a `CompatibilityPinOpenerV1`. Its order is fixed:

1. Invoke `canonicalize_lenient(root)`.
2. Return without opening or scanning if canonicalization fails.
3. Only after successful canonicalization, construct `CompatibilityCheckedScanSourceV1` with the supplied opener.
4. Invoke `scan_checked_rows_with_source` on the canonical root.
5. Return without assessment if source open fails.
6. Iterate the retained rows by reference, preserving the existing legacy, custody, and unreadable-custody decision behavior.
7. Emit the existing event unchanged for every retained row:
   `tracing::info!(record = path, ?decision, "made exact-absence decision");`.
8. Make the private engine result available to module tests after assessment without changing the public function’s unit return.

Production `sweep_orphans_with_exact_absence` delegates to this helper with `FilesystemCompatibilityPinOpenerV1` and discards its private result.

This is the deterministic report-side pin-failure seam. It is below supplied-root canonicalization and replaces only the custody pin-open result. It retains production `canonicalize_lenient`, `read_dir`, iterator behavior, shared selection, legacy reads, custody reads, engine ordering, decisions, and events.

The same-root conformance test must invoke:

- `sweep_orphans_with_exact_absence_with_pin_opener`; and
- `scan_worktree_records_with_pin_opener`;

with the identical root spelling and equivalent opener outcomes.

Compare ordered projections as follows:

- compare the same selected display paths in the same order;
- compare valid legacy `WorktreeSidecar` values by structural equality;
- compare valid decoded `WorktreeCustodyRecordV1` values by structural equality before the report route’s decision projection;
- compare unreadable custody results by exact `CustodyReadRefusalV1` equality;
- compare omission of malformed legacy and unrelated names;
- compare the exact not-pinnable refusal on deterministic pin failure.

The public report route does not expose a complete decoded custody record. The private report-side result is therefore the required comparison point; asserting decoded custody equivalence from a decision alone is insufficient.

The report-only exact `OsString`, iterator-error count, and root-observation set have no matching public action projection and are not falsely declared equal. Test exact-name retention directly on the engine row. Test iterator-error accounting separately with an injected source session. A2a does not project either detail publicly.

Tests invoking the engine or helpers directly are not enough to prove production delegation. Record a symbol-scoped source audit showing:

- `sweep_orphans_with_exact_absence → sweep_orphans_with_exact_absence_with_pin_opener → scan_checked_rows_with_source`;
- `scan_worktree_records → scan_worktree_records_with_pin_opener → scan_checked_rows_with_source`;
- both production helpers construct `CompatibilityCheckedScanSourceV1`;
- no second production session-driving loop exists.

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

The refactor must preserve eager ordering already present at the base: the scan drains and reads before the exact route assesses or logs its rows. A2a installs the engine representation of that ordering but adds no report-level ordering claim beyond current behavior.

### Enumeration-descriptor ownership decision

A2a deliberately retains `std::fs::ReadDir` in the compatibility session to preserve behavior. It does not claim that `ReadDir` exposes an inspectable identity for the directory object being enumerated.

The meaning of `RootObservationSetV1::retained_enumeration_object` is fixed now:

> The field may contain an identity only when it was captured from the exact retained directory descriptor whose duplicated descriptor drives name enumeration. Identity read from the root path, from the separate custody pin, or from a descriptor that did not drive enumeration does not satisfy the field.

Accordingly:

- A2a production leaves `retained_enumeration_object` as `None`;
- A2a production leaves all three root observations as `None`;
- A2a production `finish` returns `RootObservationSetV1::default()`;
- A2a does not present `std::fs::ReadDir` as retained identity evidence;
- A2a does not populate the field from `PinnedDirectoryV1`, because that is the separate custody-read descriptor;
- A2a does not weaken the field to mean path metadata observed near enumeration.

A2b must replace the A2a `ReadDir` storage on the required Unix lanes with a bridge-core retained-directory enumerator that:

- opens and retains one directory descriptor independently of the custody pin opener;
- enumerates names from a duplicate of that same descriptor;
- exposes metadata from the retained descriptor for `retained_enumeration_object`;
- preserves the independent custody pin-failure behavior;
- preserves raw-root alias acceptance for the action projection;
- leaves the observation unavailable on a target where descriptor-owned enumeration cannot be provided without changing scan behavior.

A2b must reserve a distinct 140-counted-line worksheet row for that bridge-core enumerator, worktree integration, and focused tests. That budget is not part of A2a and may not be borrowed to extend A2a. This decision resolves the ownership seam without fabricating root evidence from `ReadDir`.

### Tracing infrastructure decision

A2a changes no event and requires no scoped tracing assertion. Its tests do not inspect event level, message, fields, count, or order.

Do not add `tracing-subscriber` to `bridge-worktree` in A2a and do not add a public reporter or global test subscriber. The existing exact-absence event remains byte-for-byte in production source and is covered only as source-preservation evidence in this refactor.

A2b owns scoped, panic-safe tracing capture when it adds report population and event-order evidence. A2b must explicitly authorize the required dev-dependency or shared test utility at that time.

A2a may add only `libc.workspace = true` under `[dev-dependencies]`, for filesystem fixture attestation on macOS. The resulting dev-dependency set is exactly `bridge-coordinator`, `bridge-controller`, and `libc`.

### Attested fixture-root mechanism

Same-root conformance must not infer a filesystem row from the default temporary-directory location.

Add a private test utility with this input contract:

- `A2A_SCAN_FIXTURE_ROOT`: an operator-owned, initially empty directory used for one conformance run;
- `A2A_SCAN_FIXTURE_LABEL`: the exact evidence label, restricted to `macos-apfs` or `ubuntu-ext4`;
- `A2A_SCAN_EXPECTED_FILESYSTEM`: the exact expected filesystem type, restricted to `apfs` or `ext4` as selected by the label;
- `A2A_SCAN_EXPECTED_MOUNT_ID`: the exact mount identity captured by the read-only preflight.

The utility must reject a missing, empty, relative, nonexistent, non-directory, nonempty, or symlink-spelled fixture root before creating any fixture entry.

Define one machine-readable attestation record containing:

- schema `a2a-scan-fixture-attestation-v1`;
- fixture label;
- canonical fixture root;
- operating-system family;
- exact filesystem type;
- exact mount identity;
- root device and inode when the platform exposes them.

Derive the values independently:

- on macOS, obtain the filesystem type and filesystem/mount identity from `statfs` through `libc`;
- on Linux, invoke `findmnt` for the supplied fixture root, require exactly one selected mount row, and read its exact filesystem type and numeric mount ID;
- on any other target, refuse the real-fixture evidence path as unsupported.

Do not accept an operator-supplied filesystem label or mount ID without deriving and comparing the actual value. Linux ext-family magic alone is insufficient because it does not distinguish ext4 from earlier ext variants.

Provide two ignored real-fixture tests:

1. `attested_scan_fixture_preflight`
   - performs only read-only attestation;
   - emits exactly one JSON attestation record with `--nocapture`;
   - creates, removes, or changes no fixture entry;
   - allows the operator to obtain and pin the exact mount identity.

2. `compatibility_and_exact_scans_conform_on_attested_same_root`
   - re-derives attestation before creating fixtures;
   - requires exact agreement with the supplied label, filesystem type, and mount identity;
   - emits the verified JSON record before fixture creation;
   - runs the full same-root conformance matrix;
   - re-derives attestation after cleanup and requires the same filesystem type and mount identity;
   - emits a final machine-readable completion record tied to the same attestation.

Synthetic tests must cover missing input, malformed input, label/type disagreement, filesystem mismatch, mount-identity mismatch, multiple Linux mount rows, unsupported target behavior, and preflight-before-mutation ordering.

The real conformance test is run once on an attested APFS fixture and once on an attested ext4 fixture. These are A2a scan-refactor rows, not A2b’s birthtime-capability or complete platform matrix. Overlayfs cannot substitute for ext4. A missing attestation utility, missing platform facility, mismatched filesystem, mismatched mount, or unavailable required fixture leaves the corresponding row pending; it must not be relabeled green.

### Same-root conformance matrix

Using one attested readable directory, identical root spelling, and equivalent pin-opener outcomes, prove:

| Fixture | Required equivalence |
|---|---|
| Valid matching legacy sidecar | Both projections select the same display path and structurally equal sidecar. |
| Malformed legacy sidecar | Both projections omit it. |
| Valid custody record | Both projections select the same display path and retain structurally equal decoded `WorktreeCustodyRecordV1` values before report assessment. |
| Malformed custody record | Both projections retain the same decode-refusal classification. |
| Over-bound custody record | Both projections retain `OverBound`. |
| Symlinked custody entry | Both projections retain the same unreadable refusal. |
| Directory-shaped custody entry | Both projections retain the same unreadable refusal. |
| Multiply-linked custody entry | Both projections retain `MultiLink`. |
| Unrelated filename | Both projections omit it. |
| Pin failure with valid legacy and custody names | Both preserve the legacy row and retain the custody row with the exact not-pinnable refusal. |

Permission-dependent unreadability is supplementary and cannot replace deterministic entry-kind, symlink, over-bound, multi-link, decode, or pin-open evidence.

Do not rely on unlink-and-recreate inode reuse. Where distinct objects matter, create both objects concurrently before any rename.

### Required tests and evidence classification

Use these test names or equally specific names preserving the stated evidence:

| Required test | Evidence against untouched `c637e493` | Production mutation caught |
|---|---|---|
| `compatibility_open_refusal_never_calls_pin_opener` | New-private-seam mechanism evidence; the seam does not compile on the base | Calling the custody pin opener before successful `read_dir`, or treating pin failure as source-open failure |
| `compatibility_pin_failure_preserves_legacy_and_refuses_custody` | New-private-seam compatibility evidence | Suppressing all rows on pin failure, refusing legacy reads, or losing the exact not-pinnable refusal |
| `checked_scan_reads_each_selected_name_before_next_and_finishes_once` | New-private-seam ordering evidence | Prefetching the next name, duplicating `finish`, skipping `finish`, or reading outside the engine |
| `checked_scan_counts_iterator_errors_and_continues_in_order` | New-private-seam status evidence | Stopping at the first item error, counting a custody refusal as an iterator error, or reordering successful rows |
| `checked_scan_silently_omits_bad_legacy_and_retains_bad_custody` | Refactor characterization; equivalent base behavior is green | Emitting malformed legacy rows, dropping custody-named refusals, or coupling decode refusal to iterator status |
| `both_production_scans_delegate_to_one_checked_scan_engine` | Production-route source and runtime evidence; the engine does not exist on the base | Leaving a disconnected helper, retaining the old loop, or creating divergent production scanners |
| `report_side_pin_failure_uses_the_post_canonicalization_opener_seam` | New-private-seam routing evidence | Hardcoding `FilesystemCompatibilityPinOpenerV1` on the exact route, consulting the opener before canonicalization, or testing only the action projection |
| `scan_worktree_records_preserves_raw_root_and_public_projection` | Base-green characterization | Canonicalizing the action root, changing its vector shape, or exposing iterator/root details |
| `exact_route_preserves_canonical_scan_root_and_unit_return` | Base-green characterization plus compiler evidence | Passing the raw root to exact enumeration or changing the public return type |
| `compatibility_and_exact_scans_conform_on_attested_same_root` | Refactor conformance evidence; no genuine runtime red | Drifting selection, omission, decoded custody content, refusal classification, ordering, or pin-failure policy between projections |
| `checked_scan_retains_exact_non_utf8_name_internally` | New-private-seam exact-name evidence | Reconstructing the engine name from lossy display text |
| `attested_scan_fixture_preflight_refuses_unverified_filesystem_or_mount` | Test-infrastructure mechanism evidence | Running a named APFS or ext4 row on an unverified default, overlay, or different mount |
| Existing sweep and custody-lock tests | Existing base-green regression evidence | Changing legacy deletion, V3 protection, run-end handling, custody locking, or public scanner consumers |

There are no A2a genuine runtime-red tests. Report all A2a tests honestly as one of:

- base-green characterization;
- new-private-seam mechanism evidence;
- production-route structural evidence;
- attestation-mechanism evidence;
- refactor conformance evidence;
- compiler API-preservation evidence.

Every new test must document the production mutation it catches.

The injected engine suite must additionally cover:

- source-open refusal;
- zero pin calls on source-open refusal;
- complete enumeration;
- injected `Ok, Err, Ok, Err` producing two iterator errors and two ordered rows;
- ignored names;
- malformed legacy omission;
- custody refusal retention without increasing iterator errors;
- exact non-UTF-8 name retention on Unix;
- `finish` after `None`;
- `finish` exactly once;
- default root observations;
- both projections discarding root observations;
- both public functions retaining their existing return types.

The APFS and ext4 conformance runs must use the same fixture definitions and assertions. Platform-specific setup may differ only in attestation.

Run and report:

- `cargo fmt --all -- --check`;
- `cargo clippy --workspace --all-targets --locked -- -D warnings`;
- `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast`;
- `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast`;
- the ignored attestation preflight and conformance test on macOS/APFS with `--nocapture`;
- the ignored attestation preflight and conformance test on Ubuntu/Linux ext4 with `--nocapture`.

Report test totals as test binaries plus doc-test suites. Do not double-count filtered subprocess output. If a required environment or facility is unavailable, report the exact exclusion; do not fabricate a completed row or install dependencies through the network.

### Deferred to A2b

- **F4 — reproducible behavioral-red control.** A2a has no genuine runtime red because it preserves both public behavior and the unit return. When A2b changes the return type, it must supply a frozen test-only patch against an exact recorded base tree, record that tree’s identity and patch diff, and run the genuine-red controls reproducibly before relying on them.
- **F6 — source-incompatible public return change.** A2a makes no public break. A2b’s planned change from `()` to `ExactAbsenceSweepReportV1` remains source-incompatible for unit-constrained callers at workspace version `0.3.1`. A2b must resolve and record the release-version boundary as a blocking pre-publication obligation rather than relying only on handoff prose.
- **F8 — birthtime-capability result visibility.** A2a adds filesystem and mount attestation, not the birthtime-capability row. A2b must make the observed `Some` or `None` result visible through a `--nocapture` probe or machine-readable artifact; a passing test whose captured output does not reveal the observed branch is insufficient.
- **F9 — possible versus guaranteed resolver observations.** A2b’s mutation inventory must distinguish possible call edges from observations guaranteed on every comparator result. `compare_path_identities` installs `deepest_existing_path` as its resolver, but an unavailable initial resolution can return `CannotProve` before the final stability-bracket calls. The final calls must not be listed as unconditional observations.

### Sizing and mandatory pre-edit stop

Use this deterministic counted-line metric:

1. Measure the final A2a tree against base `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`.
2. For every owned changed file, count each added nonblank physical line after the fmt gate.
3. A replacement counts its added side. Deleted lines do not consume the cap.
4. Exclude only the complete pinned Rust seam block above when it is byte-identical. Any alteration removes the exemption.
5. Imports, attributes, declarations, macro invocations, assertions, comments, parameterized rows, test utilities, and nested constructs count by their nonblank added lines.
6. Assign every counted line to exactly one worksheet row by file and purpose.
7. If a line plausibly spans purposes, assign it to the first applicable row from top to bottom.
8. `git diff --numstat` may corroborate changed files but is not the final count because it includes blank and exempt lines.

There is no contingency row and no borrowing between rows.

Pre-edit worksheet:

| Counted component | Pre-edit estimate | Counted-line cap |
|---|---:|---:|
| Freely authored `checked_scan.rs` production implementation outside the pinned block | 90 | 105 |
| `sweep.rs` engine, shared classifier, private exact helper, and action projection | 145 | 165 |
| Checked-scan injected-source and ordering tests | 95 | 110 |
| Same-root conformance, routing, public-projection, and exact-name tests | 220 | 250 |
| Filesystem and mount attestation utility plus synthetic and ignored-fixture tests | 105 | 125 |
| `crates/bridge-worktree/Cargo.toml` dev-dependency line | 1 | 5 |
| **Total counted lines** | **656** | **760** |

The excluded pinned block does not consume a worksheet row. The 760-line cap is a strict subset of A2’s former 1,650-line cap and remains in A1’s measured scale.

Before editing, re-estimate every row against the exact base. Stop if any row or the total will exceed its cap. Report the revised estimates and propose a narrower follow-up split; do not compress tests, attestation, declarations, or structural evidence and do not silently extend A2a.

### Falsification license

Every symbol, caller, matrix row, and behavioral statement in this task is an anchored claim against `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`. The repository remains authoritative.

If the base identity differs; the worktree is not clean; either public signature differs; `scan_worktree_records` does not use `read_dir` before pin opening; `read_sidecar` does not silently omit failures; `read_custody_record_in` does not retain the stated refusals; the exact and action routes do not enumerate canonical and raw root spellings respectively; the current exact route does not drain before assessment; the existing event differs; the report surface, readiness gate, or constructor allowances differ; the dev-dependency set differs; bridge-core already exposes a cross-crate retained-descriptor enumerator suitable for this source; a listed production caller differs; the pinned seam cannot land byte-for-byte; or any conformance expectation is wrong, record the exact source evidence and stop before editing.

Do not adapt the implementation around a false anchor. Finding the work smaller is acceptable. The A2a/A2b split, unit-return boundary for A2a, T3a-decides/T3b-acts boundary, and T3b action-time re-decision remain settled.

## Acceptance Criteria

1. Work begins only from exact clean base `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`, after factual and sizing checks.
2. `crates/bridge-worktree/src/sweep/checked_scan.rs` exists and contains the complete pinned seam byte-for-byte.
3. The pinned seam passes rustfmt unchanged; no line is re-normalized.
4. The temporary module-level `dead_code` allowance is justified only by A2b-pending root declarations.
5. The concrete compatibility session remains private to `checked_scan.rs`.
6. The compatibility source calls `read_dir` before the custody pin opener and refuses source open only when `read_dir` fails.
7. Pin failure preserves legacy reads and retains custody names with the exact not-pinnable refusal.
8. Production `finish` returns `RootObservationSetV1::default()`.
9. A2a does not call the root classifier or populate any root observation.
10. One shared display classifier preserves the base legacy and custody selection rules.
11. One production-bound `scan_checked_rows_with_source` exclusively owns `next_name` → immediate selected read → `finish`.
12. Iterator-item errors are counted and skipped; custody refusals remain rows and malformed legacy sidecars remain omitted.
13. Exact `OsString` names survive engine enumeration without reconstruction from lossy display text.
14. Both production projections delegate to the same engine and real compatibility source.
15. The private exact-route opener seam is below canonicalization and permits deterministic report-side pin failure.
16. Same-root conformance compares full legacy values, full decoded custody values, exact custody refusals, ordering, omission, and deterministic pin-failure policy.
17. `scan_worktree_records` retains its public signature, eager vector projection, raw-root behavior, flattened iterator errors, and all existing callers.
18. `sweep_orphans_with_exact_absence` retains its public unit-returning signature, canonical scan root, existing decision behavior, and unchanged event.
19. `sweep_orphans` retains its statement-position exact call, independent guard, warning, early return, canonical `root_cwd` decisions, and raw action-scan argument.
20. No public report type, constructor allowance, readiness rule, iterator, API test, or report documentation changes in A2a.
21. No tracing capture, public reporter, global subscriber, or `tracing-subscriber` dependency is added.
22. `crates/bridge-worktree/Cargo.toml` adds only `libc.workspace = true` under dev-dependencies.
23. The fixture preflight independently verifies and records filesystem type and mount identity before conformance fixture creation.
24. A named APFS or ext4 row cannot pass on an unattested, mismatched, overlay, or different mount.
25. The same-root conformance matrix passes on separately attested macOS/APFS and Ubuntu/Linux ext4 fixtures.
26. No test is labeled genuine runtime red; evidence classifications remain exact.
27. The source audit proves both public routes reach the engine and no second production session-driving loop remains.
28. The `retained_enumeration_object` meaning is preserved as identity from the exact enumeration descriptor, and A2a does not fabricate it from `ReadDir`, a path, or the custody pin.
29. A2b’s retained-descriptor enumerator obligation and distinct 140-line budget remain recorded.
30. No ownership, locking, transition, publication, settlement, deletion, prune, rename, backend-cleanup, or T3b authority is introduced.
31. Fmt, clippy, the full locked workspace suite, the package suite, and both attested conformance rows are reported with exact totals or explicit exclusions.
32. Every counted worksheet row and the 760-line total remains within cap.
33. The resulting A2a commit is suitable as the accepted base for A2b without changing either public scan signature.

## Files

- `crates/bridge-worktree/src/sweep.rs`
  - declare the private checked-scan module with the temporary A2b-pending allowance;
  - add the private intermediate row and engine-result types;
  - extract the shared display classifier;
  - implement `scan_checked_rows_with_source`;
  - implement `scan_worktree_records_with_pin_opener`;
  - delegate the public action scanner through the engine;
  - implement `sweep_orphans_with_exact_absence_with_pin_opener`;
  - delegate the public unit-returning exact route through that helper;
  - preserve all exact-decision, event, action, guard, removal, and run-end behavior;
  - add routing, projection, conformance, exact-name, and fixture-attestation tests.
- `crates/bridge-worktree/src/sweep/checked_scan.rs`
  - create with the complete pinned seam block;
  - implement the private `ReadDir`-backed compatibility session;
  - implement the real compatibility source;
  - preserve independent custody pin failure;
  - return default root observations;
  - add focused injected-source and ordering tests.
- `crates/bridge-worktree/Cargo.toml`
  - add `libc.workspace = true` under `[dev-dependencies]` for macOS fixture attestation;
  - do not add `tracing-subscriber` or any other dependency.
- `crates/bridge-worktree/src/sweep/report.rs`
  - read-only reference for the pinned seam and A1 surface;
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
  - read-only reference for `PinnedDirectoryV1`, descriptor reads, the existing crate-private enumerator, and the deferred comparator inventory;
  - do not modify in A2a.
- `Cargo.toml`
  - read-only workspace dependency and version reference;
  - do not modify.
- `Cargo.lock`
  - no dependency-resolution change is expected because `libc` is already a locked workspace dependency;
  - do not modify unless the falsification license stops the task.
- `rust-toolchain.toml`
  - read-only pinned-toolchain reference;
  - do not modify.
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs`
  - read-only public API reference;
  - do not modify in A2a.
- `bin/a2a-bridge/src/main.rs`
  - read-only caller-audit reference;
  - no CLI changes.

## Spec Refs

Authoritative at base commit `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`:

- `Cargo.toml`
- `Cargo.lock`
- `rust-toolchain.toml`
- `crates/bridge-worktree/Cargo.toml`
- `crates/bridge-worktree/src/sweep.rs`
- `crates/bridge-worktree/src/sweep/report.rs`
- `crates/bridge-worktree/src/provider_path.rs`
- `crates/bridge-worktree/src/custody.rs`
- `crates/bridge-worktree/src/custody_lock.rs`
- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs`
- `bin/a2a-bridge/src/main.rs`

## Commit Message

refactor(worktree): unify exact and action scans

Add one production-bound compatibility checked-scan engine and route both the
unit-returning exact-absence path and the existing action scanner through it.

Preserve raw action-root behavior, canonical exact-root behavior, legacy
omission, custody refusals, eager ordering, and public return types, while adding
deterministic pin-failure, same-root conformance, and fixture-attestation
evidence for the A2b report work.
