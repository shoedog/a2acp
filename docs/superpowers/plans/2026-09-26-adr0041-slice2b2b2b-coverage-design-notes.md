# ADR-0041 Slice 2B2b2b — coverage design notes (pre-spec)

**Status:** design notes only; this is not yet a task. The 2B2b2b task spec is written after 2B2b2a (the walker)
merges, against its API.
**Source:** Sol spec review round 1 of the combined 2B2b2 revision 1 (`4dc44191`, REJECT, raw result SHA-256
`dac57b65…`), plus owner decisions of 2026-09-26.

## Owner decisions carried forward

- **Class scope.** The common-clone set: `refs_and_head`, `index`, `stash_and_reflogs`, `in_progress_git_operations`,
  `git_configuration_and_hooks`, `worktree`, and `bridge_evidence`. Evidence of linked worktrees, nested repositories
  or submodules, LFS, or alternates makes that class `unresolved`, and the unit parks.
- **Worktree.** Capture everything except a root `target/` under `cargo-target-v1` (the root must have regular files
  `Cargo.toml` and `Cargo.lock`).
- **Split.** 2B2b2 is delivered as 2B2b2a (the walker) followed by 2B2b2b (this document's scope).
- **Submodule, LFS, and graft detection.** Park on:
  - any `.gitmodules`;
  - any `filter=lfs` in a worktree `.gitattributes` or in `info/attributes`;
  - `info/grafts`;
  - index gitlinks, detected **exactly** by adding one read-only `ls-files --stage` command to 2B2a's closed
    `GitCommandV1` set. 2B2b2b specifies that as a small, reviewed seam amendment of `custody_git.rs`, with its own
    closed argv, environment, and bounds, and runner controls in 2B2a style.

## Round-1 findings owned by 2B2b2b, and the intended resolution

1. **Bind the plan to the manifest (#1, and walker-lineage round 2 #4).** The capability stores the plan's canonical
   coverage rows **and** its exclusion and dependency records. Before any scratch write, it requires exact equality
   of **all three collections** (coverage, exclusions, dependencies) with the manifest's. This adds read-only
   canonical `exclusions()` and `dependencies()` accessors to `CustodyManifestV1`. RED: change only one dependency
   digest, and the export refuses before any write. Receipts bind one-to-one to the captured non-object classes. RED: a
   captured-plan row against an empty-manifest row, and the reverse, both refuse before any write.
2. **Whole-subtree ownership (#2).** Each class owns whole subtrees; no subtree is split between classes.
   - `refs/`, **including** `refs/stash`, belongs to `refs_and_head`. The stash is a ref.
   - `logs/`, including `logs/refs/stash`, belongs to `stash_and_reflogs`.
   - `info/`, **including** `sparse-checkout`, belongs to `git_configuration_and_hooks`.

   This satisfies ADR-0041's coverage table, because the table requires capture, not a particular class boundary.
   A cross-class **multiset** control asserts that every source entry appears exactly once across all frames. Its keys
   are `(RootDomain, lossless_path)`, where `RootDomain` is `RepositoryRoot` or `GitDirectoryRoot` (walker-lineage
   round 2 #5). A worktree `HEAD` and a git-directory `HEAD` are then distinct, and an external or separate git
   directory is handled. Tests cover such a collision and a `--separate-git-dir` repository.
3. **The root `.git` connector (#3).** The worktree frame emits the root `.git` entry itself: a directory as
   `IncludeEntryOnly` (a directory entry with its mode and no children), or a gitfile as ordinary regular-file bytes.
   The git-directory classes own its contents, relative to the git directory. The multiset control counts `.git`
   exactly once.
4. **Exclusion records (#4).** The plan returns canonical `CustodyCoverageEntryV1` rows, plus
   `CustodyExclusionV1("cargo-target-v1")` and `CustodyDependencyV1` records that bind `Cargo.toml` and `Cargo.lock`,
   and `rust-toolchain.toml` when present, by SHA-256. Any existing manifest-validation requirement is met. RED:
   the plan builds a valid manifest, and changing the `Cargo.lock` bytes invalidates the dependency.
5. **Dependency evidence without storage (#6).** Covered by the owner decision above. Every detection has a
   fixture that parks even when the `.git` storage directory is absent.
6. **The object-database row (#7).** It is never walked and never receipted. The row derives only from the manifest
   inventory and 2B2's verified pack path: `captured` if the inventory is non-empty, else `empty`. RED: an empty
   initialized repository and a one-object repository.

## Also required

- The class tables and the coverage-state and reason mapping onto the existing 14-code `CustodyReasonCodeV1`, carried
  from combined revision 1 §3–§4, with the subtree changes above applied.
- Staged-frame export (`work/payload-<code>.frame`, a ledger reservation, a pre-seal receipt comparison, and sealing
  from the retained descriptor), carried from combined revision 1 §5.
- Lock files and `gc.pid` park with `WriterUncontrolled`, and `shallow` parks, as in combined revision 1.
- **Walker concurrency contract** (2B2b2a implementation review, deferred). Class walks must not run concurrently
  over one shared `PinnedDirectoryV1`, because `list_child_names` duplicates the pin's descriptor and shares its
  directory offset. Either walk classes sequentially, or pin each class root separately.

## 2B2b2b split (owner-approved 2026-09-27)

2B2b2b is delivered as:
- **2B2b2b1:** the coverage plan. Its task is `docs/superpowers/plans/2026-09-27-adr0041-slice2b2b2b1-coverage-plan-task.md`.
- **2B2b2b2:** capability binding and staged-frame export. It is specified after 2B2b2b1 merges, against its real
  plan type.

2B2b2b2 must include:
- **Three-collection binding (#1).** The capability stores the plan's coverage, exclusions, dependencies, and
  receipts. Before any scratch write, all three collections must equal the manifest's exactly, which requires
  read-only `exclusions()` and `dependencies()` accessors on `CustodyManifestV1`. Receipts must map one-to-one onto
  captured, walked classes, and the plan's generation must equal the capability's.
- **Bounded staged writes (2B2b2b round 1 #4).** The staged frame is written through an exact-length bounded writer
  capped at the receipt's frame length. Any attempted extra byte is `SourceDrift`, so actual staged bytes never
  exceed the ledger reservation. RED: grow a planned file past its receipt with no ledger headroom.
- **Staged-frame export:** reserve, create `work/payload-<code>.frame`, `recheck()`, re-walk sequentially with a
  fresh pin, compare `(frame_length, frame_sha256, inventory_digest)` with the receipt, then seal from the retained
  descriptor. Its controls cover binding refusals, drift, round trip, and regressions.
- **Mandatory mount-point census** (owner ruling 2026-09-27, from the 2B2b2b1 implementation review round 2).
  Before any plan is exported, enumerate the host's mount points (Linux `/proc/self/mountinfo`, macOS `getfsstat`).
  Park the unit with `MountBoundary` if any mount point lies inside the source repository or the git directory. A
  same-device bind mount is invisible to `st_dev` checks, and can alias the git directory beneath `target/` (a false
  `cargo-target-v1` exclusion). RED: a Linux same-device bind-alias fixture on a mount-capable lane, plus a
  deterministic census seam.
