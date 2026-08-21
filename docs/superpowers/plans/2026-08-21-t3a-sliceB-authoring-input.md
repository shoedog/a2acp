---
task-type: implement
---

# Author the slice B task spec — populate the root observations

## Description

Write a complete, dispatchable **task spec** for slice B of the R2f1b 3d T3a
lane. Not a design note, not a plan — a spec an implementer can execute.

T3a increment 1 is complete and merged. `sweep_orphans_with_exact_absence`
returns a populated `ExactAbsenceSweepReportV1`, but every report it produces
carries `CustodyRootObservationV1::Unavailable` for its root, because production
never captures the three root identities the classifier needs.

**Slice B closes exactly that hole.**

### A naming collision you must resolve, not inherit

The A2a-1 spec assigned this work to "A2b". The A2b slice as actually dispatched
and merged deliberately did **not** do it — A2b changed the return type and
explicitly left root population to a later slice. So the label moved and the
obligation did not.

Call this work **slice B** throughout, and state in the spec that the A2a-1
text's references to "A2b" for descriptor-owned enumeration mean this slice. Do
not leave two names for one obligation.

### The single production hole

`crates/bridge-worktree/src/sweep/checked_scan.rs`:

```rust
    fn finish(self: Box<Self>) -> RootObservationSetV1 {
        RootObservationSetV1::default()
    }
```

That is `CompatibilityCheckedScanRootSessionV1::finish`. Everything downstream
already works and is tested:

- `RootObservationSetV1` carries three `Option<RootIdentityCaptureV1>` fields:
  `retained_enumeration_object`, `pinned_custody_directory`, `final_named_root`.
- `RootIdentityCaptureV1` carries `dev`, `ino`, `birthtime`, each `Option`.
- `classify_root_observations` in `crates/bridge-worktree/src/sweep.rs` requires
  **three complete `(dev, ino, birthtime)` tuples**; any absent capture or
  incomplete tuple yields `Unavailable`, and `Unavailable` outranks a mismatch.
  It deliberately does **not** use `DirectoryIdentityV1::matches`, whose
  absent-birthtime wildcard would weaken the proof. A2b already landed
  `root_observation_classifier_*` tests covering pinned, identity-changed, and
  incomplete cases.

So slice B does not design a classifier. It supplies real captures to one that
already exists and is proven.

### The field's meaning is already settled — do not weaken it

Carried verbatim from the A2a-1 spec:

> The field may contain an identity only when it was captured from the exact
> retained directory descriptor whose duplicated descriptor drives name
> enumeration. Identity read from the root path, from the separate custody pin,
> or from a descriptor that did not drive enumeration does not satisfy the
> field.

This is why A2a left it `None` rather than filling it from `std::fs::ReadDir` or
from `PinnedDirectoryV1`: `ReadDir` exposes no inspectable identity for the
object it enumerates, and the custody pin is a different descriptor.

### What the implementer must build

A bridge-core retained-directory enumerator that:

- opens and retains one directory descriptor, independently of the custody pin
  opener;
- enumerates names from a **duplicate** of that same descriptor;
- exposes metadata from the retained descriptor for
  `retained_enumeration_object`;
- preserves the existing independent custody pin-failure behavior;
- preserves raw-root alias acceptance for the action projection;
- leaves the observation unavailable on any target where descriptor-owned
  enumeration cannot be provided **without changing scan behavior**.

That last clause is a hard constraint, not an escape hatch: slice B must not
change what the scan selects, omits, or decides. Every behavior A2a-2's ten
characterization scenarios pinned must still hold.

### The birthtime problem is real and must be handled explicitly

`BirthTimeV1::from_metadata` in `crates/bridge-core/src/fs_custody.rs` is
`metadata.created().ok().and_then(...)`, and `Metadata::created()` errors on
platforms and filesystems without creation-time support. So a complete tuple is
not always obtainable, and `Pinned` is not always reachable — by design.

Two consequences the spec must address:

1. Declare the supported filesystem-capability boundary. On a birthtime-less
   filesystem the correct result is `Unavailable`, and that is a **supported
   outcome**, not a failure.
2. A capability test that passes for either `Some` or `None` proves nothing if
   the observed branch is invisible in captured output. Require a targeted
   `--nocapture` probe or a machine-readable artifact recording the fixture
   identity, the observed capability, and the resulting classifier expectation.
   This is inherited finding **F8**, deferred from A2a and now due.

### Scope fences

Slice B does **not**:

- set `EXACT_ABSENCE_POLICY_READY_V1` to `true` — that gate and the
  population-admission rule belong to increment 2;
- change `sweep_orphans_with_exact_absence`'s signature or the report vocabulary;
- add ownership, locking, transition, unlink, or removal authority. **T3a decides
  and reports; T3b acts.** A later actor must re-open, re-read, re-bind, and
  re-prove exact absence under its own lock regardless of what the report says;
- repair the Unix-only separator guard in `is_custody_record_name`, which A2a-2
  characterized deliberately and left unrepaired.

### Sizing

A2a-1 reserved **140 counted lines** for the bridge-core enumerator, its worktree
integration, and focused tests, and said that budget may not be borrowed. Treat
140 as the anchor for the enumerator itself and size the rest of the slice
honestly on top of it.

Counted lines are added nonblank physical lines after the fmt gate, one row per
line, no contingency, no borrowing. Per-test cost measured in this crate is
roughly 28 nonblank lines — use that, not a guess. If your honest estimate is
much larger than the reserved anchor, say so and propose a split rather than
compressing evidence to fit; this lane has twice paid for caps that were
estimated instead of measured.

### Operator-owned gates

The implement container's egress cannot fetch the pinned `a2a-lf` dependency, so
`cargo` cannot build there. The implementer makes the implementation-candidate
commit and authors a handoff carrying six unticked `PENDING OPERATOR` gate lines;
the host operator runs the gates and makes the handoff-only evidence commit.
Reporting a gate as blocked is correct behavior; inventing a total is not.

### Environment facts

Your working tree is at `main`, and the repository is authoritative over every
claim above. Read the code — `checked_scan.rs`, `sweep.rs`, `fs_custody.rs` — and
verify each anchor before relying on it. Do not restate this brief as though you
had observed it.

You cannot read anything outside the repository. The spec you emit must never
name a path outside the repository, because the implementer runs in a container
with only the code tree mounted.

## Acceptance Criteria

This authoring task is complete when the emitted spec satisfies the output
contract below: correct sections, a descriptor-ownership requirement that cannot
be satisfied by path metadata or the custody pin, the capability boundary and F8
visibility requirement, each scope fence stated once, a per-row sizing worksheet,
and a falsification license.

## Output contract

Emit the complete slice B task spec, with:

- front matter `task-type: implement`;
- `## Description`, `## Acceptance Criteria`, `## Files`, `## Spec Refs`,
  `## Commit Message`;
- the descriptor-ownership requirement stated precisely enough that an
  implementer cannot satisfy it with path metadata or the custody pin;
- the filesystem-capability boundary and the F8 visibility requirement;
- the scope fences above, each stated once;
- a sizing worksheet with per-row caps;
- a falsification license: the repository is authoritative, and an implementer
  who finds a stated anchor false must stop and report rather than adapt.

Prefer precision over length. A spec that an implementer can execute without
guessing beats a longer one that restates context.
