---
task-type: implement
---
# Implement ADR-0041 Slice 2B2b2b2: plan-to-manifest binding, mount census, and bounded staged-frame export

**Revision:** 1 (draft for spec review)
**Implementation base:** current `main`; bind the exact SHA at dispatch. The predecessor is 2B2b2b1, PR #119 at
`e5de184b`.
**Parent plan:** `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md`. The serial order is
2B2b2b1 → **2B2b2b2** → 2B3. This is the last child of 2B2b.
**Design notes:** `docs/superpowers/plans/2026-09-26-adr0041-slice2b2b2b-coverage-design-notes.md`, the 2B2b2b2
section, including the owner-mandated mount-point census.

## 1. Description

### 1.1 Goal

A capability minted from a 2B2b2b1 coverage plan exports a sealed capsule. Each common-clone class payload in it is
the frame of exactly that class's planned entries, and:
- the plan is bound exactly to the manifest the capsule seals;
- no mount point hides inside the protected set;
- a source change between planning and export is refused, never sealed;
- staged bytes never exceed their scratch-ledger reservation.

Nothing is silently dropped, and no write lands outside the scratch root.

### 1.2 Decisions in force

- **Owner, 2026-09-26/27:** the common-clone set; `cargo-target-v1`; the plan/export split; and the **mandatory
  mount-point census**, which closes the same-device bind-mount alias deferred from 2B2b2b1 review round 2.
- **Standing:** a hostile same-user racer is out of scope: detect and refuse. 2B2a, 2B2b1, 2B2b2a, and 2B2b2b1 are
  consumed unchanged, except for the additions listed in §7.

### 1.3 Non-scope

This child does not include production quiescence or minting (the fixture mint remains the only mint), restore
(2B3), wiring, or the classes that remain unresolved in v1.

## 2. Walk recipes and restaging (`custody_coverage.rs`)

- **Recipes.** For each receipt, the plan retains a private **walk recipe**: the root domain (`RepositoryRoot` or
  `GitDirectoryRoot`), the exact selection parameters the planner used (the git-directory class set, and the
  worktree's pinned git-directory identity and `cargo_target_excluded`), the frame header, the frame budget, and the
  entry budget.
- **Restaging.** Add `restage_class_v1(plan, sources, class, sink: &mut dyn Write) -> Result<WalkReceiptV1,
  CustodyWalkErrorV1>`. It re-runs **exactly** the recipe's walk with a fresh pin of the recipe's root into `sink`.
  The planner remains the only owner of selection logic, and the exporter never reconstructs a selection.
- **Sequencing.** Restaging runs sequentially; it never shares a pin concurrently (the 2B2b2a contract).

## 3. Capability binding (`custody_export.rs`, `custody_seal.rs`)

### 3.1 Minting

Add `CustodyCaptureCapabilityV1::from_fixture_quiescence_with_plan(decision, manifest, sources, plan)`, which is
`#[cfg(test)]` like the existing fixture mint. The capability stores the plan and its sources, and each receipt
becomes a `Walked { class, generation_id, length, sha256 }` stream. The existing fixture-stream form is unchanged.

### 3.2 Binding checks

These run in order, **before any scratch write**. Each failure is a typed `CapabilityBinding` refusal, which leaves
no scratch entry.

1. **Unresolved rows.** Any `unresolved` row in the plan refuses. 2B1 layout would also refuse it.
2. **Generation.** The plan's generation must equal the capability's and the manifest's.
3. **Coverage, exclusions, dependencies.** Each of the three collections must equal the manifest's exactly,
   canonical order included. This adds read-only `exclusions()` and `dependencies()` accessors to
   `CustodyManifestV1`, with no validation change.
4. **Receipts.** Receipts map one-to-one onto the manifest's `captured` classes other than `object_database`.
5. **Mount census (§4).**

## 4. Mount-point census (new `custody_mounts.rs`, `#[cfg(unix)]`)

### 4.1 Enumeration

`mount_points_v1() -> Result<Vec<Vec<u8>>, CustodyMountErrorV1>` returns canonical mount-point paths as raw bytes.

- **Linux:** read `/proc/self/mountinfo` through a bounded read (at most 4 MiB). Take field 5, the mount point, and
  decode its octal escapes (`\040`, `\011`, `\012`, `\134`) strictly. A malformed line or an oversize file refuses.
- **macOS:** `getfsstat(NULL, 0, MNT_NOWAIT)` for the count, then a second call into an exactly sized buffer, taking
  each `f_mntonname`. If the count changes between the two calls, retry once; a second change refuses. This adds an
  authorized unsafe boundary, recorded in a new-unsafe inventory control in the style of 2B2a's A14.
- **Other unix:** refuse `CensusUnsupported`, which the exporter maps to a binding refusal.

### 4.2 Check

The export refuses with `MountBoundary` if any mount point **other than the one containing the root itself** lies
strictly inside:
- the canonical source repository;
- the source git directory;
- the primary object store;
- any alternate store.

That means a canonical-path prefix on component boundaries: `/repo/target` is inside `/repo`, `/repository` is not.

A census error also refuses, **failing closed**. The census runs at binding (§3.2 step 5), and again immediately
before the first scratch write, together with `capability.recheck()`.

### 4.3 Seams

A test seam injects the mount list. Linux mountinfo parsing is tested on fixture text, including escapes and
malformed lines.

## 5. Staged-frame export

For each `Walked` class, in layout order:

1. **Reserve** the receipt's `frame_length` plus one entry allowance in the 2B2 ledger. Over budget refuses before
   anything is created.
2. **Create** `work/payload-<class-code>.frame` through the retained `work/` descriptor.
3. **Recheck.** Run `capability.recheck()` and the census.
4. **Restage.** Call `restage_class_v1` into a **bounded writer** wrapping the staged descriptor. The writer is capped
   at exactly `frame_length` bytes; an attempted write of byte `frame_length + 1` returns `SourceDrift` without
   writing it. Actual staged bytes therefore never exceed the reservation.
5. **Compare.** Require the restaged `(frame_length, frame_sha256, inventory_digest)` to equal the receipt, else
   `SourceDrift`: no seal, and a typed incomplete outcome as in 2B2.
6. **Seal.** Rewind the retained descriptor and seal through the existing `PlaintextReaderV1::File` path. The file is
   never reopened by name.

Staged frames are removed with `work/`.

## 6. Acceptance criteria

1. **Binding.** Each §3.2 step has refusal controls, each proved by a scratch snapshot showing no entry created:
   - a plan with an unresolved row;
   - a generation mismatch;
   - a coverage row flipped in each direction;
   - an extra exclusion, and a missing one;
   - one dependency digest changed;
   - a receipt missing, and an extra receipt.
2. **Census.**
   - The injected mount list parks the unit (no entry created) for a mount point at the repository's `target`, and
     for one inside the git directory and one inside an alternate store.
   - A mount point that is a prefix sibling (`/repository` versus `/repo`) does not park.
   - The Linux mountinfo parser handles escapes, and refuses malformed lines and oversize input.
   - macOS `getfsstat` returns a list containing `/` on the host lane.
   - A census error fails closed.
   - **Real bind mount:** on a mount-capable Linux lane, bind-mount a directory containing the git directory at
     `repo/target` and require a refusal. Where no mount-capable lane is available, this is a named exclusion, and
     the injected-seam control stands in for it.
3. **Bounded staging.**
   - A planned file grown past its receipt, with **no** ledger headroom: the export refuses `SourceDrift`, and the
     staged file length is ≤ the reservation. A seam proves the bounded writer rejected the overflow byte.
   - A planned file changed at the same length refuses through the digest comparison.
4. **End to end.** From a rich non-bare clone: plan → manifest (built from the plan's three collections) →
   capability → export → sealed capsule.
   - The clone has a dirty worktree, untracked and ignored files, a stash, reflogs, an in-progress merge with rerere,
     hooks, bridge evidence, and a Cargo `target/`.
   - 2B1 binding validates the capsule, and every coverage payload decodes, with the 2B2b1 decoder, to exactly that
     class's walked entries.
   - The capsule holds the Git pack, and no `target/**` bytes appear in any payload.
5. **Restage identity.** `restage_class_v1` on an unchanged source reproduces the receipt byte-for-byte. A walk
   recipe changed through a test seam produces a different receipt.
6. **Regressions.** Every 2B2, 2B2a, 2B2b1, 2B2b2a, and 2B2b2b1 control still passes, including the fixture-stream
   exports.

## 7. Owned paths

- `crates/bridge-core/src/custody_coverage.rs` and its tests: walk recipes and `restage_class_v1` only;
- `crates/bridge-core/src/custody_export.rs` and its tests: §3, §5, and the census call sites;
- `crates/bridge-core/src/custody_seal.rs`: two read-only accessors;
- `crates/bridge-core/src/custody_mounts.rs` (new) and its tests;
- `crates/bridge-core/src/lib.rs`: the module declaration;
- `crates/bridge-core/src/custody_git_tests.rs`: only if the A14 inventory must register the new unsafe sites;
- `docs/superpowers/reviews/<date>-adr0041-slice2b2b2b2-implementation-handoff.md`.

## 8. RED-first, mutation, and verification

As in earlier children:
- a structural RED, a behavioral RED per control, and a persisted foreground mutation matrix. At minimum, it has one
  row for each binding step, each census rule (including the prefix-boundary check and fail-closed), the second
  census call, the bounded-writer cap, the receipt comparison, restaging through a reconstructed selection instead of
  the recipe, and reopening by name;
- byte-exact restores and a snapshot proof;
- gates: workspace `--all-targets` and default, both `--no-fail-fast`; workspace clippy with `-D warnings`; fmt; diff
  check; hygiene; and `cargo deny` where installed;
- lanes: the container with the proxy variables unset, native ext4 in CI, and the controller's macOS lane (which
  proves the `getfsstat` census).

## 9. Stop conditions

Stop and report if any of the following is needed:
- a change to `custody_frame.rs`, `custody_walk.rs`, `fs_custody.rs`, or `custody_git.rs`;
- a selection built outside `custody_coverage.rs`;
- a new manifest field or reason code;
- following a symlink;
- a path-addressed source read;
- a new dependency.

Also stop on an open-class review population, or a review cap exhausted without convergence.

## 10. Review

The spec and implementation reviews each have a two-round cap, run by Sol/xhigh in hard read-only mode. Findings are
tagged WRONG or SMELL and MATERIAL or IMMATERIAL to §1.1. Implementation and repairs are by Opus 5.5 through
`a2a-bridge implement`, and the PR merges on approval and green CI.

## 11. Commit Message

feat(bridge-core): ADR-0041 Slice 2B2b2b2 plan binding, mount census, and bounded staged-frame export
