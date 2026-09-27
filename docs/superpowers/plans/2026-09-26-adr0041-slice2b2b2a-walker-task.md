---
task-type: implement
---
# Implement ADR-0041 Slice 2B2b2a: the descriptor-relative no-follow walker

**Revision:** 3 (folds lineage round 2, see §11). Revision 1 was the combined 2B2b2 task (Parts A and B). Sol spec review round 1 rejected it and
recommended splitting; the owner approved the split on 2026-09-26. This revision is Part A alone, with the
walker-side findings folded (§11).
**Implementation base:** current `main`; bind the exact SHA at dispatch. The predecessor is 2B2b1, PR #114 at
`b8d2686d`.
**Parent plan:** `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md`, amended 2026-09-26. The
serial order is 2B2b1 → **2B2b2a** → 2B2b2b → 2B3.
**Sibling:** 2B2b2b (class tables, coverage plan, manifest binding, and exporter staged frames) consumes this API
unchanged. Its design notes are in `docs/superpowers/plans/2026-09-26-adr0041-slice2b2b2b-coverage-design-notes.md`.

## 1. Description

### 1.1 Goal

A class-agnostic walker that turns one pinned directory subtree into one 2B2b1 frame. It is descriptor-relative
and never follows a symlink, and it applies a caller-supplied selection. Every entry beneath the root is:
- emitted exactly once;
- skipped, because the selection says so;
- or it refuses the walk with a typed error that 2B2b2b maps to a park reason.

A concurrent change to the tree is detected as drift, never silently absorbed.

**Guarantee boundary.** This is detection, not prevention. A change is detected if it is visible in any emitted
entry's `(kind, dev, ino, mode, size, mtime, ctime)`, or in any directory's child-name set, at any point between the
entry's first observation and the walk's final verification pass (§4.6). Coherence of the capture rests on the
capability's quiescence decision (ADR-0041 §8: a coherent snapshot, or exclusive managed-writer quiescence). A writer
that forges timestamps to hide a change is a hostile same-user racer, which the owner has ruled out of scope. The walker never crosses a device
boundary. It never writes except into the caller's frame encoder.

### 1.2 Non-scope

This child does not include:
- which files belong to which class (2B2b2b);
- coverage states, manifests, or exporter changes (2B2b2b);
- restore (2B3);
- any change to `custody_git.rs`, `custody_frame.rs`, or `custody_export.rs`.

The walker is crate-private and `#[cfg(unix)]`, and nothing in production uses it until 2B2b2b.

## 2. New `PinnedDirectoryV1` primitives (`fs_custody.rs`)

These are `pub(crate)` and `#[cfg(unix)]`, inside `fs_custody`'s existing authorized unsafe boundary.

1. **`list_child_names(remaining: &mut u64, label) -> Vec<ChildBytesV1>`.** Reads entries through a **duplicate** of
   the retained descriptor (`dup`, then `fdopendir`, `readdir` to EOF, and `closedir`). It skips `.` and `..`, and
   returns each name's exact bytes.
   - **Budget.** `remaining` is **one global entry budget** shared by the whole walk. Each name read is charged
     before it is stored. Reaching zero refuses `EntryLimit` before storing another name, so the total memory held
     across every live directory list is bounded by the one budget (§4, SMELL-1 fold).
2. **`child_metadata_no_follow(name, label) -> ChildStatV1 { kind, mode, size, dev, ino, mtime_ns, ctime_ns }`.**
   Uses `fstatat(fd, name, AT_SYMLINK_NOFOLLOW)`. `kind` is one of `Directory`, `Regular`, `Symlink`, or
   `Special(type)`, and `mode` is the full `st_mode & 0o7777`.
3. **`read_child_symlink(name, max_bytes, label) -> Vec<u8>`.** Uses `readlinkat` into a buffer of `max_bytes + 1`,
   and refuses if the target does not fit. It never truncates.
4. **`current_metadata(label) -> ChildStatV1`.** `fstat` of the pinned directory's own retained descriptor. It
   returns the same fields as `child_metadata_no_follow`, and is used for directory drift checks (fold of lineage
   round 2 #3).

Existing, unchanged, reused:
- **`open_existing_child_directory`** opens subdirectories no-follow.
- **`open_regular_file`** opens content. It uses `open_child_no_follow` (`O_RDONLY | O_CLOEXEC | O_NOFOLLOW |
  O_NONBLOCK`), so a FIFO swapped in refuses instead of blocking.

Both validate names with the byte-level `validated_child_name`, which rejects only empty, `.`, `..`, `/`, and NUL,
so non-UTF-8 names pass losslessly.

A new-unsafe-site inventory control (in the style of 2B2a's A14) enumerates the added `unsafe` blocks and pins their
count and files.

## 3. Walker API (new module `custody_walk.rs`)

```text
walk_tree_v1(root: &PinnedDirectoryV1,
             selection: &dyn WalkSelectionV1,
             entry_budget: u64,
             encoder: CustodyFrameEncoderV1<W>) -> Result<WalkReceiptV1, CustodyWalkErrorV1>

trait WalkSelectionV1 {
    fn decide(&self, path: &CustodyFramePathV1, stat: &ChildStatV1) -> WalkDecisionV1;
}

enum WalkDecisionV1 {
    Include,        // emit; recurse into a directory
    IncludeEntryOnly, // emit this entry; a directory is emitted with no children and never opened
    Skip,           // emit nothing, do not recurse (the caller accounts for it)
    Park(WalkParkV1), // refuse the walk with this caller-typed reason
}
```

- **Paths.** `path` is root-relative and becomes the frame path. The root itself is implicit, as in 2B2b1.
- **Skip.** A `Skip` decision is the caller's responsibility. 2B2b2b uses it only for entries another class owns or
  that are policy-excluded, and it must account for each one. The walker counts skipped entries in its receipt, so
  the caller can prove its accounting.
- **`IncludeEntryOnly`.** This lets 2B2b2b emit a connector entry, such as the worktree's root `.git` directory,
  without descending into it.
- **Encoder ownership (fold of lineage round 2 #2).** The walker takes the encoder **by value**, calls `finish()` after
  the final verification pass succeeds, and returns its summary. Callers keep ownership of the sink by passing a
  borrowed writer as `W` (for example `&mut File`).
- **Receipt.** `WalkReceiptV1` holds the 2B2b1 frame summary returned by `finish()` (entries, content bytes, frame
  bytes, frame SHA-256), plus `skipped_entries` and the §4.6 inventory digest.
- **Selector purity.** `decide` must be a pure function of `(path, stat)`. The walker evaluates it only after sorting,
  so decisions and the first reported `Park` are independent of `readdir` order.
- **Errors.** `CustodyWalkErrorV1` is typed:
  - `Frame(CustodyFrameErrorV1)`, which wraps path-length, budget, and mode refusals;
  - `EntryLimit`;
  - `UnsupportedEntry { path, kind }`;
  - `MountBoundary { path }`;
  - `SourceDrift { path, detail }`;
  - `Park(WalkParkV1)`;
  - `Io`.

  After an error, no receipt is returned, and the caller must discard the sink (as 2B2b1's encoder poisoning
  requires).

## 4. Traversal semantics

1. **Order.** For each directory:
   - list its children with `list_child_names`, charging the global budget;
   - **sort all names by byte order**;
   - then, in that order, get each child's `child_metadata_no_follow` and apply `decide`;
   - process the kept names in order, recursing into an `Include` directory before its next sibling.

   This depth-first pre-order is exactly 2B2b1's component-wise order. At most one name list per depth level is live
   at a time, and all of them share the one global budget.
2. **Entry kinds.**
   - **Directory:** `encoder.directory(path, stat.mode)`, where `mode` is the full `& 0o7777` value, per the 2B2b1
     caller contract. The encoder refuses special bits.
   - **Regular file:** `open_regular_file`, then `fstat` the opened descriptor. Its `(dev, ino, size, mtime, ctime)`
     must equal the pre-open `fstatat`, otherwise `SourceDrift`. It then streams exactly `size` bytes into
     `encoder.regular_file`, and afterwards `fstat`s again. A change in size, mtime, or ctime is `SourceDrift`.
   - **Symlink:** `read_child_symlink(name, 4095)`, then `encoder.symlink`. The target is never followed or opened.
   - **Special** (FIFO, socket, block or character device): refuse `UnsupportedEntry`.
3. **Device boundary (fold of #8).** **Every** entry's `stat.dev`, including regular files and symlinks, must equal
   the root's `dev`. Otherwise it is `MountBoundary`. A regular file's opened-descriptor `dev` is checked again under
   rule 2. Bind-mounted files and directories are both caught.
4. **Directory drift (fold of #5).** Before listing, `fstat` the directory's own descriptor: `(dev, ino, mtime,
   ctime)`. After its entire subtree has been emitted:
   - re-list the directory through a new duplicate;
   - require the **exact same child-name set**;
   - require that each kept child's `(kind, dev, ino)` is unchanged;
   - `fstat` the directory again, and require its `(dev, ino, mtime, ctime)` to be unchanged.

   Any difference is `SourceDrift`. A file created, deleted, or renamed in a directory during the walk is therefore
   detected, even when the directory inode is unchanged.

   The re-listing charges a **separate** verification counter that is capped at the same budget, and it does not
   draw on the global emit budget.
5. **Descent identity.** After opening a child directory, its descriptor's `current_metadata()` must equal the
   listing's `fstatat` in `(dev, ino, mode, mtime, ctime)`. Otherwise it is `SourceDrift`: a directory replaced, or
   `chmod`ed, between listing and descent. The emitted mode is the opened descriptor's mode.
6. **Final verification pass (fold of lineage round 2 #1).**
   - **During the walk,** every emitted entry's `(frame path, kind, dev, ino, mode, size, mtime, ctime)` is folded
     into a running SHA-256, the **inventory digest**. Skipped entries are folded in the same way, with a skip marker.
   - **After the last entry,** and before `finish()`, the walker re-walks the tree **stat-only**: the same
     descriptor-relative traversal and the same selection, but with no content reads and no encoding. It recomputes
     the inventory digest and requires equality.
   - **Effect.** A change to any entry after it was first observed, such as an earlier sibling edited while a later
     file is being read, is detected as `SourceDrift`. Memory stays O(depth), because only the running digest and the
     current path stack are held.
   - **Budget.** The verification re-walk re-lists each directory through a fresh duplicate, under the same global
     budget ceiling as a separate counter. The peak live name memory is therefore at most **two budgets' worth**: one
     list per depth level from each pass, or from the §4.4 re-listing. It is stated and tested at that bound (fold of
     SMELL-2).
7. **Root.** The walker takes an already pinned `PinnedDirectoryV1`. `PinnedDirectoryV1::open` canonicalizes before
   pinning, so a symlinked root path resolves to its target. Whether a root may be reached through a symlink at all
   is the pinning caller's policy (2B2b2b), not a walker claim (fold of #9). The walker only checks that the root's
   `(dev, ino)` is unchanged at the start and the end of the walk.

## 5. Acceptance criteria

Each item has a dedicated control over real temp-directory trees, unless a seam is named.

1. **Order.** For a tree mixing directories, files, and symlinks, including `a`, `a/b`, and `a.b` and non-ASCII
   names, the emitted frame decodes (with the 2B2b1 decoder) to exactly the tree's entries in canonical order.
2. **Entry kinds.** The following are captured correctly:
   - an empty directory;
   - a zero-length file;
   - a multi-chunk file (over 64 KiB);
   - an executable file;
   - a symlink to an absolute path outside the root, to `..`, and a dangling symlink, all captured as symlinks.

   A test seam counting `openat` targets proves nothing outside the root is opened.
3. **Special files.** A FIFO and a Unix socket each refuse with `UnsupportedEntry`. A setuid file and a sticky
   directory each refuse through the encoder's `UnsupportedMode`.
4. **Selection.**
   - `Skip` omits an entry and does not descend into it, and `skipped_entries` counts it.
   - `IncludeEntryOnly` emits a directory with no children and never opens it (the seam proves no `openat`).
   - `Park` refuses with the caller's reason.
5. **Device boundary.** Using an injected-`dev` seam (real mounts are unavailable in tests), a different `dev` on a
   subdirectory, on a regular file, and on a symlink each refuses with `MountBoundary`. A regular file whose opened
   `dev` differs from its listing refuses too.
6. **Drift.** Each of the following refuses with `SourceDrift`, using hook seams between steps:
   - an **earlier sibling's content edited after its post-read check**, while a later file is still being read, which
     the §4.6 final pass catches (the lineage round 2 #1 regression);
   - a directory `chmod`ed between listing and descent (§4.5);
   - a file created after its directory's first listing (the review's #5 regression, with the inode unchanged);
   - a file deleted after listing;
   - a directory replaced between listing and descent;
   - a file whose size changes while it is read;
   - a file replaced between `fstatat` and open.
7. **Budget.**
   - The global entry budget at max succeeds and max+1 refuses with `EntryLimit`.
   - A counting seam proves the peak live name count never exceeds two budgets.
   - A wide parent plus a wide child, with a small budget, refuses without the second list reaching full size. A
     counting seam on name allocations proves this.
   - Frame budget refusals propagate as `Frame(ByteBudget)`.
8. **Non-UTF-8 names.** On Linux only (`cfg(target_os = "linux")`), a non-UTF-8 name round-trips. macOS APFS refuses
   such names, which is a named exclusion.
9. **Case-insensitive filesystem (macOS).** The walker emits the on-disk spelling and never folds case.
10. **Determinism.** Two walks of an unchanged tree produce identical receipts. A seam that reverses `readdir` order
    produces the same receipt and the same first `Park`.
11. **Receipt.** For an empty tree and for a non-empty tree, the returned summary equals the summary obtained by
    decoding the finished sink with the 2B2b1 decoder.

## 6. RED-first and mutation evidence

- A structural RED on the predecessor, and a behavioral RED per control.
- A persisted, foreground mutation matrix with one row per guard, at minimum:
  - the order sort removed;
  - no-follow off (following symlinks);
  - the special-file refusal off;
  - the device check off, once for each of directories, files, and symlinks;
  - the pre-open versus opened identity check off;
  - the during-read drift check off;
  - the post-subtree re-listing check off;
  - the descent identity check off;
  - the global budget off (a per-list budget instead);
  - the `IncludeEntryOnly` handling descending anyway;
  - the `Skip` counter off;
  - the final verification pass off;
  - the descent mode comparison off;
  - the decision made before sorting.
- Byte-exact restores with a fresh mtime, and a snapshot proof after the matrix.

## 7. Verification

The parent plan §8 gates:
- `cargo test` on the workspace, both with and without `--all-targets`, using `--no-fail-fast`;
- workspace clippy with `-D warnings`, `fmt --check`, `git diff --check`, and repo hygiene;
- `cargo deny`, where installed; otherwise it is a named exclusion and CI runs it.

Platform lanes:
- **Container:** Linux on overlayfs.
- **Native ext4:** CI. The drift and identity controls must pass on native ext4; overlayfs does not substitute for
  it.
- **macOS:** the controller's host lane, which includes the case-insensitive filesystem control.

In the implementation container, unset `HTTP_PROXY` and `HTTPS_PROXY` for workspace runs.

## 8. Files

- `crates/bridge-core/src/fs_custody.rs`: only the four §2 methods and their unsafe inventory;
- `crates/bridge-core/src/custody_walk.rs` (new) and `custody_walk_tests.rs` (new);
- `crates/bridge-core/src/lib.rs`: the `#[cfg(unix)] #[allow(dead_code)] mod custody_walk;` declaration only;
- `docs/superpowers/reviews/<date>-adr0041-slice2b2b2a-implementation-handoff.md`.

## 9. Stop conditions

Stop and report if any of the following is needed:
- a change to `custody_git.rs`, `custody_frame.rs`, or `custody_export.rs`;
- following a symlink;
- a path-addressed read where a descriptor-relative one is required;
- a new dependency;
- anything outside §8.

Also stop if the review finds an open-class population, or if the review cap is exhausted without convergence.

## 10. Review

The spec review and the implementation review each have a two-round cap. This revision is the first on the split
child. It counts as spec round 2 of the original 2B2b2 lineage (under the convergence rule, a successor inherits
the spent budget). Findings are tagged WRONG or SMELL and MATERIAL or IMMATERIAL to §1.1. Implementation and repairs
are by Opus 5.5 through the bridge, and the PR merges on approval and green CI.

## 11. Review history

**Round 1**, on combined revision 1 (`4dc44191`), gave REJECT, raw result SHA-256 `dac57b65…`. This child folds the
walker-side items:
- **#5:** same-inode directory additions. Fixed by post-subtree re-listing and a name-set comparison (§4.4).
- **#8:** bind-mounted regular files. Fixed by checking `dev` on every entry and on the opened descriptor (§4.3).
- **#9:** the symlink-root claim was impossible under canonicalizing pins. The claim is removed; admission policy
  belongs to the caller (§4.6).
- **SMELL-1:** enumeration memory. Fixed by one global entry budget (§2.1, §4.1).
- **SMELL-2:** the split itself, now owner-approved.

Items #1, #2, #3, #4, #6, and #7 belong to 2B2b2b and are carried in its design notes.

## 12. Commit message

```text
feat(bridge-core): ADR-0041 Slice 2B2b2a descriptor-relative no-follow walker
```

**Lineage round 2** (the first review of this split child, on revision 2 at `b2ebc4a1`): REJECT.
- **Resolved from round 1:** #5, #8, #9, SMELL-1, and SMELL-2.
- **New walker blockers,** folded in revision 3:
  - #1: late mutation of an already-visited entry. Closed as a class by the §4.6 final stat-only verification pass
    and inventory digest, with the guarantee boundary stated in §1.1.
  - #2: encoder ownership. The walker now takes the encoder by value and finishes it.
  - #3: the missing descriptor metadata primitive, added as `current_metadata`.
- **SMELLs folded:** decisions are made after sorting and the selector is pure; the two-budget peak is stated and
  tested; the parent plan's stale order line is corrected.
- **Carried to the 2B2b2b design notes:** #4 (dependency equality) and #5 (the cross-root namespace).

Revision 3 is reviewed in one disclosed extension round, under the owner's authorization while converging.
