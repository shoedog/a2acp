---
task-type: implement
---
# Implement ADR-0041 Slice 2B2b2: the no-follow walker, coverage classification, and staged-frame export

**Revision:** 1 (draft for spec review)
**Implementation base:** current `main` after PR #114 (2B2b1 merged at `b8d2686d`) and the docs reconciliation PR #115.
The implementor binds the exact SHA at dispatch.
**Parent plan:** `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md` (amended 2026-09-26: 2B2b1 →
2B2b2 → 2B3)

## 1. Description

### 1.1 Goal

2B2 seals non-Git coverage classes from opaque, fixture-only in-memory streams, and 2B2b1 defines the frame that a
coverage payload is made of. This child connects them to real files: for an ordinary implementation clone, the
exporter seals each covered class as a 2B2b1 frame of exactly the entries that class owns. Every entry of the git
directory and the worktree must be either owned by exactly one class, recorded as excluded-reproducible under a named
policy, or cause the unit to **park** (a typed refusal).

Nothing is silently dropped, and no write lands outside the scratch root.

### 1.2 Owner decisions (2026-09-26)

- **Class scope (the "common-clone set").** v1 captures `refs_and_head`, `index`, `stash_and_reflogs`,
  `in_progress_git_operations`, `git_configuration_and_hooks`, `worktree`, and `bridge_evidence`.
  - `object_database` stays with 2B2's Git pack.
  - Any evidence of `linked_worktrees`, `nested_repositories_and_submodules`, `lfs_and_external_payloads`, or
    `alternates_and_shared_stores` makes that class `unresolved`, which parks the unit (ADR-0041 §4).
  - `external_evidence` is a caller-declared input (§4.4).
- **Worktree.** Capture every tracked, untracked, and gitignored entry, with one exception. A worktree-root `target/`
  directory is recorded as `reproducible_outputs = excluded_reproducible` under policy `cargo-target-v1`, provided the
  worktree root also has a `Cargo.toml` and a `Cargo.lock`. The Cargo.lock digest is the reconstruction dependency.
- **Standing rulings.** A hostile same-user racer is out of scope: detect and refuse, as in 2B2 §2. The 2B2a seam and
  the 2B2b1 frame are consumed unchanged.

### 1.3 Non-scope

This child does not include:
- production quiescence or capability minting, or any CLI, config, store, or operator wiring (the capability keeps
  its crate-private fixture mint);
- manifest assembly beyond the coverage rows (§4);
- the six classes that v1 leaves unresolved, beyond detecting them;
- restore (2B3);
- encryption;
- any change to `custody_git.rs` or `custody_frame.rs`.

## 2. Part A: descriptor-relative walker

### 2.1 New `PinnedDirectoryV1` primitives (`fs_custody.rs`)

Add three `pub(crate)`, `#[cfg(unix)]` methods, inside `fs_custody`'s existing authorized unsafe boundary:

1. **`list_child_names(limit, label) -> Vec<ChildBytesV1>`.** Reads the directory's entries through a **duplicate**
   of the retained descriptor (`dup` + `fdopendir`, then `readdir` until the end, then `closedir`).
   - It skips `.` and `..`, and returns each name's exact bytes.
   - It refuses `EntryLimit` once `limit + 1` names have been read, before allocating any further name.
   - The names are raw bytes (`OsStr` as bytes), not `ChildNameV2`, because captured names are lossless (2B2b1 §2.3).
2. **`child_metadata_no_follow(name, label) -> ChildStatV1 { kind, mode, size, dev, ino, mtime_ns, ctime_ns }`.**
   Uses `fstatat(fd, name, AT_SYMLINK_NOFOLLOW)`. `kind` is one of `Directory`, `Regular`, `Symlink`, or
   `Special(type)`.
3. **`read_child_symlink(name, max_bytes, label) -> Vec<u8>`.** Uses `readlinkat`, with a buffer of `max_bytes + 1`.
   If the target does not fit in `max_bytes`, it refuses; it never truncates.

Two existing methods complete the set, and both are already suitable:
- `open_existing_child_directory` opens subdirectories;
- `open_regular_file` opens file content.

Both go through `open_child_no_follow` (`O_RDONLY | O_CLOEXEC | O_NOFOLLOW`, with `O_NONBLOCK` for regular files,
so a FIFO swapped in refuses instead of blocking). Both validate names with the byte-level `validated_child_name`,
which rejects only empty, `.`, `..`, `/`, and NUL, so non-UTF-8 names pass losslessly.

A new-unsafe-site inventory control, like 2B2a's A14, enumerates the added `unsafe` blocks and pins their count.

### 2.2 The walker (new module `custody_walk.rs`, `#[cfg(unix)]`)

`walk_class_tree(root: &PinnedDirectoryV1, selection, budgets, encoder: &mut CustodyFrameEncoderV1<W>) ->
WalkReceiptV1`. It emits one class's entries, in 2B2b1 canonical order, into a caller-owned frame encoder.

- **Order.**
  - For each directory, the walker lists its children, applies the selection (§3), and sorts the kept names by byte
    order.
  - It then processes them in order. A kept directory is recursed into before its next sibling, which yields
    depth-first pre-order. This is exactly 2B2b1's component-wise order.
  - At most one name list per depth level is alive at a time. The number of levels is bounded by the 4096-byte frame
    path limit, and each list by the entry budget.
- **Entry kinds.**
  - **Directory:** `encoder.directory(path, mode)`, where `mode` is `st_mode & 0o7777`, per the 2B2b1 §2.4 caller
    contract.
  - **Regular file:** open it no-follow and stream it into `encoder.regular_file` with the `fstat` size as the
    declared length. Then `fstat` the open descriptor again. If size, mtime, or ctime changed, refuse with
    `SourceDrift`.
  - **Symlink:** `read_child_symlink`, then `encoder.symlink`. The target is never followed and never opened.
  - **Special:** refuse `UnsupportedEntry` (FIFO, socket, device), which parks the unit.
- **No mount crossing.** Each directory's `dev` must equal the class root's `dev`. A different `dev` refuses
  `MountCrossing` and parks the unit.
- **Identity per directory.** The walker records each directory's `(dev, ino)` when it opens the directory. After
  listing, it rechecks that the parent's child `fstatat` still reports the same `(dev, ino)`. A mismatch is
  `SourceDrift`.
- **Errors.** Frame errors (`PathTooLong`, `UnsupportedMode`, budgets, and so on) propagate as typed walker errors.
  The walker maps each of them to a coverage **reason** code (§4.2); it never skips an entry.
- **Receipt.** `WalkReceiptV1` returns the 2B2b1 frame summary: entries, content bytes, frame bytes, and frame
  SHA-256.

### 2.3 Hashing sink

`walk_class_tree` is generic over the encoder's `W: Write`. The planning pass (§4) uses a counting, hashing sink
that stores nothing. The export pass (§5) uses the staged scratch file.

## 3. Part B: class selection tables

Selections name entries by exact byte names relative to a class root. The git directory is `source_git_dir`, and
the worktree is `source_repository`, both from the capability. A bare source has no worktree, so the `worktree`
class is `empty`.

### 3.1 Git-directory top-level classification (closed table)

Every top-level entry of the git directory is classified by this table. An entry the table does not name is
`unresolved` (in `git_configuration_and_hooks`) with reason `ContentUnresolved`, and the unit parks.

| Entry (exact name, or prefix where marked `*`) | Class | Notes |
|---|---|---|
| `HEAD`, `ORIG_HEAD`, `FETCH_HEAD`, `packed-refs` | refs_and_head | |
| `refs/`, **except** the `refs/stash` file | refs_and_head | the subtree walk skips `stash` directly under `refs/` |
| `refs/stash` | stash_and_reflogs | |
| `logs/` | stash_and_reflogs | the whole subtree |
| `index`, `sharedindex.*` | index | |
| `info/sparse-checkout` | index | the `info/` walk routes this one name to `index` |
| `info/` (other entries) | git_configuration_and_hooks | |
| `config`, `config.worktree`, `description`, `hooks/`, `branches/`, `remotes/`, `rr-cache/` | git_configuration_and_hooks | |
| `MERGE_HEAD`, `MERGE_MSG`, `MERGE_MODE`, `MERGE_AUTOSTASH`, `AUTO_MERGE`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `REBASE_HEAD`, `SQUASH_MSG`, `COMMIT_EDITMSG`, `BISECT_*`, `rebase-merge/`, `rebase-apply/`, `sequencer/` | in_progress_git_operations | |
| `a2a-bridge/`, `A2A_*` | bridge_evidence | the clone's in-repository bridge evidence |
| `objects/` | object_database | not walked here; 2B2's pack covers it. A non-comment line in `objects/info/alternates` makes `alternates_and_shared_stores` **unresolved** |
| `worktrees/` | linked_worktrees | **unresolved** if it has any entry, else `empty` |
| `modules/` | nested_repositories_and_submodules | **unresolved** if it has any entry |
| `lfs/` | lfs_and_external_payloads | **unresolved** if it has any entry |
| `shallow`, `*.lock` (any top-level lock file), `gc.pid` | none | **unresolved**, with reason `ContentUnresolved` (shallow) or `WriterUncontrolled` (locks, gc) |
| `gc.log` | git_configuration_and_hooks | inert log |

Lock files deeper in the tree (for example `refs/heads/main.lock`) also park with `WriterUncontrolled`.

`refs/` routing: the walk of `refs/` excludes exactly the regular file `refs/stash`, which is routed to
`stash_and_reflogs`. All other `refs/` entries (heads, tags, remotes, notes, replace, custom) stay in
`refs_and_head`.

### 3.2 Worktree

- **Root.** The class root is `source_repository`, when it differs from `source_git_dir`.
- **Excluded from the worktree class:**
  1. the root entry `.git`, whether a directory or a gitfile; it belongs to the git-directory classes;
  2. under `cargo-target-v1`, the root directory `target`, when the root has both a regular-file `Cargo.toml` and a
     regular-file `Cargo.lock`.
- **Captured:** every other entry, at any depth, including any directory named `target` below the root.
- **Nested repositories.** A `.git` entry below the root (a directory or a file) makes
  `nested_repositories_and_submodules` unresolved (reason `DependencyUnresolved`), and the unit parks. The walker
  still stops descending into that subtree.
- **Gitfile worktree.** If the root `.git` is a gitfile (a linked worktree or a `--separate-git-dir` checkout), v1
  requires `source_git_dir` to be the capability's pinned git directory, and it makes no further claim. A
  `gitdir:` target the capability does not pin makes `linked_worktrees` unresolved with `DependencyUnresolved`.

### 3.3 Excluded-reproducible record

When `cargo-target-v1` applies, the coverage row is `reproducible_outputs = excluded_reproducible` with
`exclusion_id = "cargo-target-v1"`. The manifest's existing `CustodyCoverageEntryV1` requires an exclusion id for this
state, and allows no reasons.

- **Policy definition.** The policy id is versioned and fixes the policy completely: exactly the worktree-root
  directory `target`, excluded only when the root has regular files `Cargo.toml` and `Cargo.lock`.
- **Dependency.** The reconstruction dependencies (`Cargo.toml`, `Cargo.lock`, and any `rust-toolchain.toml`) are
  ordinary worktree entries, so they are **captured in the worktree payload**. That binds them without a new
  manifest field.

When the policy does not apply, `reproducible_outputs` is `empty`. The walker never descends into an excluded
`target`.

## 4. The coverage plan

### 4.1 API

`plan_coverage_v1(request) -> CustodyCoveragePlanV1` is crate-private and `#[cfg(unix)]`.
- **Input:** the capability's pinned `source_repository` and `source_git_dir`, the generation id, the frame
  budgets, and the external-evidence declaration (§4.4).
- **Output:** one row for each of the 14 classes, `(class, state, reasons)`, plus one `CustodyClassReceiptV1 { class,
  frame_length, frame_sha256 }` per captured class.

The plan performs **one read-only walk per class** into the hashing sink. It opens no file for writing, spawns
nothing, and writes nothing.

### 4.2 State derivation

Reasons use only the existing closed `CustodyReasonCodeV1` vocabulary. Per `CustodyCoverageEntryV1`, only
`unresolved` carries reasons, and it must carry at least one.

| Situation | State | Reason (existing code) |
|---|---|---|
| captured class with at least one selected entry, walked successfully | `captured` | none |
| captured class with no selected entries, or a class v1 does not capture with no evidence present | `empty` | none |
| excluded `target` under `cargo-target-v1` (only `reproducible_outputs`) | `excluded_reproducible` | none (`exclusion_id = "cargo-target-v1"`) |
| special file, special mode bits, path over 4096 bytes, frame over budget, §3.1 unknown git-directory entry, or `shallow` | `unresolved` | `ContentUnresolved` |
| a top-level or nested `*.lock` file, or `gc.pid` | `unresolved` | `WriterUncontrolled` |
| evidence of a class v1 does not capture (non-empty `worktrees/`, `modules/`, or `lfs/`, or an alternates line), a nested `.git`, or an unpinned gitfile target | `unresolved` (the evidenced class) | `DependencyUnresolved` |
| a directory on another device | `unresolved` | `MountBoundary` |
| a file or directory changed during the walk | `unresolved` | `IdentityChanged` |

The row that goes `unresolved` is the class that owns the offending entry. For an unknown top-level git-directory
entry, which no class owns, it is `git_configuration_and_hooks`, the class of git-internal metadata.

A captured in-progress operation is **not** a capture-time park. ADR-0041 §4 holds only automatic reap for it, and
reap is outside this child.

A plan containing any `unresolved` row is still returned: the plan is evidence. It cannot be exported, because 2B1
layout construction already refuses `unresolved` coverage.

### 4.3 Receipt semantics

A receipt is the 2B2b1 `CustodyFrameSummaryV1` frame length and frame SHA-256 of the class's frame, encoded with the
header `(class, generation_id)`. Receipts bind the plan to the export (§5).

### 4.4 External evidence

`external_evidence` is a required caller input with exactly two values:
- `NoExternalEvidenceRecordedV1`, which gives `empty`;
- `ExternalEvidenceUnresolvedV1`, which gives `unresolved` with a reason.

There is no default. Both values are constructible only inside `bridge-core`. v1 tests use both; the wiring slice
decides how production derives it.

## 5. Exporter integration: staged frames

### 5.1 Capability

`CustodyCapturedStreamV1` becomes an enum:
- `Fixture { class, generation_id, length, sha256, bytes }`, the existing `#[cfg(test)]` form, unchanged;
- `Walked { class, generation_id, length, sha256 }`, whose length and SHA-256 come from a coverage-plan receipt.

The fixture mint accepts either form. `Walked` streams can be built only from a `CustodyCoveragePlanV1` whose
generation equals the capability's generation. The capability rejects a plan with any `unresolved` row.

### 5.2 Staging and sealing

`build_artifact_plan` maps a `Walked` class to a new `PlannedPlaintextV1::StagedFrame(class)`. Before sealing that
artifact, the exporter:

1. Reserves, in the 2B2 scratch ledger, the declared frame length plus one 64 KiB entry allowance. It refuses the
   reservation if that would exceed the budget.
2. Creates the new regular child `work/payload-<class-code>.frame` through the retained `work/` descriptor
   (`create_new_regular_child`).
3. Rechecks the capability (`capability.recheck()`), then re-walks the class into a frame encoder whose sink is that
   file.
4. Requires the staged frame's length and SHA-256 to equal the receipt. A mismatch is `SourceDrift`: no seal, and a
   typed incomplete outcome, as in 2B2 §5.3.
5. Rewinds the retained descriptor and seals from it through the existing `PlaintextReaderV1::File` path. The file
   is never reopened by name, exactly as the pack is sealed.

Each staged frame counts toward the 10 GiB scratch-wide ceiling (2B2 §3). A class whose frame would exceed the
remaining scratch budget refuses before any byte is written. Staged frames are removed with `work/` exactly as the
staged pack is.

### 5.3 What does not change

The following are consumed unchanged:
- publication order, the seal barrier, and receipts;
- 2B2's Git pack flow;
- the envelope port;
- every 2B2 control.

## 6. Acceptance criteria

Each item needs a dedicated control. Real-directory fixtures are built under the test temp root.

1. **Classification table.** For every §3.1 row, a git directory containing only that entry puts it in the expected
   class or state. An unlisted top-level name, a top-level `*.lock`, a nested `refs/heads/x.lock`, and `shallow`
   each park with the stated reason.
2. **`refs/stash` routing.** `refs/stash` lands in `stash_and_reflogs` and not in `refs_and_head`. All other `refs/`
   entries stay in `refs_and_head`.
3. **Worktree.**
   - Tracked, untracked, gitignored, empty-directory, executable, symlink (including a dangling one and one pointing
     outside the root), and zero-length entries are all captured.
   - The root `.git` is excluded.
   - The root `target` is excluded under `cargo-target-v1` only when both `Cargo.toml` and `Cargo.lock` are regular
     files. It is captured when either is missing, and any nested `target` is always captured.
4. **Parking.** Each of the following parks the unit with its reason, and no stream is produced for that class:
   - a FIFO;
   - a setuid file;
   - a nested `.git` (both a directory and a file);
   - non-empty `worktrees/`, `modules/`, and `lfs/`;
   - an alternates file;
   - a path longer than 4096 bytes.
5. **No-follow.**
   - A symlink to an outside directory is captured as a symlink, and nothing outside the root is opened. A test seam
     counts `openat` targets to show this.
   - A symlinked class root is refused by the pin.
6. **Mount crossing.** It parks. Tests use an injected `dev` seam, since real mounts are not available in tests.
7. **Drift.** Each of the following yields `SourceDrift` with no seal and a typed incomplete outcome:
   - a file edited between the plan and the export (a receipt mismatch);
   - a file whose size, mtime, or ctime changes while it is read;
   - a directory replaced between listing and descent.
8. **Round trip.** For a real non-bare clone fixture, decoding every sealed coverage payload with the 2B2b1 decoder
   yields exactly the entries of each class, byte-for-byte.
   - The fixture includes a dirty worktree, an untracked file, an ignored file, a stash, a reflog, an in-progress
     merge, and a hook.
   - The union of the decoded class paths, plus `objects/`, plus the excluded `target`, equals the full walk of the git
     directory and the worktree. This proves nothing was silently dropped.
9. **Determinism.** Planning the same unchanged source twice gives identical receipts.
10. **Budget.**
    - The staged frame reservation refuses at max+1 of the scratch budget before any byte is written.
    - A class over the frame budget is `unresolved`, not truncated.
11. **Non-UTF-8 names.** On Linux only (`cfg(target_os = "linux")`), a non-UTF-8 name round-trips. macOS APFS
    refuses such names, which is a named exclusion.
12. **Case-insensitive filesystem.** On macOS, a directory holding only `A`, then later only `a`, captures the
    on-disk spelling. The walker never folds case.
13. **2B2 regression.** Every existing 2B2 control still passes with fixture streams, and a new walked-stream
    variant of the 2B2 golden export seals.

## 7. RED-first and mutation evidence

As in 2B2 and 2B2b1:
- a structural RED on the predecessor;
- a behavioral RED per control;
- a persisted, foreground mutation matrix with one row per guard, at minimum:
  - each classification row mis-routed;
  - the stash routing off;
  - the lock check off;
  - the unknown-entry park off;
  - the `.git` exclusion off;
  - the `cargo-target-v1` preconditions off;
  - the nested-`.git` park off;
  - the special-file refusal off;
  - no-follow off;
  - the mount check off;
  - each drift check off;
  - the receipt comparison off;
  - the staged-frame reservation off;
  - the file reopened by name instead of the retained descriptor;
- byte-exact restores with a fresh mtime, and a snapshot proof after the matrix.

## 8. Verification

The parent §8 gates, plus the platform lanes:
- **Linux:** the container (overlayfs) and native ext4 in CI. The drift and identity controls must run on native
  ext4; overlayfs does not substitute for it.
- **macOS:** the controller's host lane, which also covers the case-insensitive filesystem control.

Use `--no-fail-fast`, unset the proxy variables for workspace runs, and record the exact totals and exclusions.

## 9. Files and stop conditions

**Owned paths:**
- `crates/bridge-core/src/fs_custody.rs`: only the §2.1 methods and their unsafe inventory;
- `crates/bridge-core/src/custody_walk.rs` (new) and `custody_walk_tests.rs` (new);
- `crates/bridge-core/src/custody_coverage.rs` (new) and `custody_coverage_tests.rs` (new);
- `crates/bridge-core/src/custody_export.rs` and `custody_export_tests.rs`: only §5;
- `crates/bridge-core/src/lib.rs`: module declarations only;
- the handoff `docs/superpowers/reviews/<date>-adr0041-slice2b2b2-implementation-handoff.md`.

**Stop conditions:** stop and report if any of the following is needed:
- a change to `custody_git.rs` or `custody_frame.rs`;
- a new manifest field or reason code (the §3.3 and §4.2 mappings use existing ones);
- following a symlink;
- a path-addressed operation where a descriptor-relative one is required;
- a new dependency;
- anything outside the owned paths.

Also stop if the review finds an open-class population, or if the diff clearly exceeds one reviewable child. In that
case, propose splitting Part A (§2) from Part B (§3–§5) rather than continuing.

## 10. Review

The spec and implementation reviews each have a two-round cap, run by Sol/xhigh in hard read-only mode. Findings are
tagged WRONG or SMELL and MATERIAL or IMMATERIAL to §1.1. Implementation and repairs are by Opus 5.5 through the
bridge, and the PR merges on approval and green CI under the owner's standing directive.

## 11. Commit message

```text
feat(bridge-core): ADR-0041 Slice 2B2b2 no-follow walker, coverage plan, and staged frames
```
