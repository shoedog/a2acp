---
task-type: implement
---
# Implement ADR-0041 Slice 2B2b2b: class coverage, manifest binding, gitlink probe, and staged-frame export

**Revision:** 1 (draft for spec review)
**Implementation base:** current `main`; bind the exact SHA at dispatch. The predecessor is 2B2b2a, PR #117 at
`f42c81fc`.
**Parent plan:** `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md`, amended 2026-09-26. The
serial order is 2B2b1 → 2B2b2a → **2B2b2b** → 2B3.
**Sources:**
- design notes `docs/superpowers/plans/2026-09-26-adr0041-slice2b2b2b-coverage-design-notes.md`;
- the combined 2B2b2 revision 1 (`4dc44191`) §3–§5, with the round-1 fixes applied;
- the merged APIs of 2B2 (`custody_export`), 2B2b1 (`custody_frame`), 2B2b2a (`custody_walk`), and 2B2a
  (`custody_git`).

## 1. Description

### 1.1 Goal

For an ordinary implementation clone, the exporter seals every class in the common-clone set as a 2B2b1 frame of
exactly the entries that class owns.
- Every entry of the git directory and the worktree is owned by exactly one class, recorded as excluded-reproducible
  under `cargo-target-v1`, or makes its class `unresolved`, which parks the unit.
- The coverage plan that produces the frames is bound exactly to the manifest the capsule seals.
- Nothing is silently dropped, and no write lands outside the scratch root.

### 1.2 Owner decisions in force (2026-09-26)

- **Common-clone set:** `refs_and_head`, `index`, `stash_and_reflogs`, `in_progress_git_operations`,
  `git_configuration_and_hooks`, `worktree`, and `bridge_evidence`.
  - `object_database` stays with 2B2's Git pack.
  - Evidence of linked worktrees, nested repositories or submodules, LFS, or alternates makes that class
    `unresolved`.
  - `external_evidence` is a caller-declared input (§4.4).
- **Worktree:** capture everything except a root `target/` under `cargo-target-v1`.
- **Gitlinks:** detected exactly, through one new read-only runner command, `ls-files --stage -z` (§2).
- **Standing rulings:** a hostile same-user racer is out of scope (detect and refuse). 2B2a, 2B2b1, and 2B2b2a are
  consumed unchanged, except for the §2 runner amendment.

### 1.3 Non-scope

This child does not include:
- production quiescence or capability minting (the fixture mint remains);
- manifest assembly beyond producing the coverage, exclusion, and dependency records (§4);
- capture of the classes that stay unresolved;
- restore (2B3);
- CLI, config, store, or operator wiring.

## 2. Runner amendment: `LsFilesStageZ` (`custody_git.rs`)

### 2.1 Command

Add `GitCommandV1::LsFilesStageZ`, with the exact argv `ls-files --stage -z`. It is read-only:
- `permits_object_store_route` is **false** for it, because it needs no objects;
- it is rooted like every other command, runs under the same closed environment, and uses the same bounded I/O.

It never touches the source repository. It runs against a **probe repository in scratch**:

1. `InitBare { dir: "work/index-probe.git" }` (the existing command, template-less).
2. Copy the source's `index` into the probe as `index`, and copy every `sharedindex.*`. The bytes are read through the
   capability's pinned `source_git_dir` descriptor with `open_regular_file`, which is no-follow, and are written with
   `create_new_regular_child` under the probe's pinned descriptor. The copied bytes and entries are reserved in the
   2B2 scratch ledger.
3. Run `LsFilesStageZ` with `GIT_DIR` set to the probe's relative name.

**Probe P1** (controller, Git 2.54.0 Apple Git-157): the copied index was listed, a gitlink appeared as
`160000 <oid> 0\t<path>\0`, the exit status was 0, and nothing was written to the probe.

A missing source `index` means no gitlinks can exist. The probe is skipped, and the result is recorded as `empty`
evidence.

### 2.2 Bounds

- **stdout limit:** `min(128 × (total copied index bytes) + 64 KiB, 256 MiB)`. This covers version-4 prefix
  compression, which is at most about 93× expansion per entry. Exceeding it is `StdoutLimit`, which makes
  `index` `unresolved` with `ContentUnresolved`.
- **stderr limit and deadline:** the existing runner policy.

### 2.3 Parser

The output is a strict sequence of records, each `^[0-7]{6} <hex oid of the source format's length> [0-3]\t<path>\0`.
- Paths must satisfy the 2B2b1 path policy.
- A malformed record, trailing bytes, or a path longer than 4096 bytes refuses. The `index` class then becomes
  `unresolved` with `ContentUnresolved`.
- Any record with mode `160000` makes `nested_repositories_and_submodules` `unresolved` with
  `DependencyUnresolved`.

### 2.4 Runner controls (2B2a style)

- The argv is exact.
- It refuses an object route.
- The environment is closed; the probe uses no `GIT_INDEX_FILE` or source path.
- At stdout max and max+1: success and `StdoutLimit`.
- The parser refuses each malformed shape.
- 2B2a's A14 unsafe inventory is unchanged; this amendment adds no unsafe code.

## 3. Class ownership

### 3.1 Git directory (root `source_git_dir`): whole-subtree ownership

Every **top-level** entry is classified by this closed table. Each class then walks the git directory root with a
selection that **Includes** its own top-level entries (whole subtrees) and **Skips** all others. The walker counts
skips; the plan proves every skip is owned by another class (§4.3).

| Top-level entry (exact name, or prefix where marked `*`) | Class |
|---|---|
| `HEAD`, `ORIG_HEAD`, `FETCH_HEAD`, `packed-refs`, `refs/` (**all** of it, including `refs/stash`) | refs_and_head |
| `logs/` (all of it, including `logs/refs/stash`) | stash_and_reflogs |
| `index`, `sharedindex.*` | index |
| `config`, `config.worktree`, `description`, `hooks/`, `info/` (all of it, including `sparse-checkout` and `attributes`), `branches/`, `remotes/`, `rr-cache/`, `gc.log` | git_configuration_and_hooks |
| `MERGE_HEAD`, `MERGE_MSG`, `MERGE_MODE`, `MERGE_AUTOSTASH`, `AUTO_MERGE`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `REBASE_HEAD`, `SQUASH_MSG`, `COMMIT_EDITMSG`, `BISECT_*`, `rebase-merge/`, `rebase-apply/`, `sequencer/` | in_progress_git_operations |
| `a2a-bridge/`, `A2A_*` | bridge_evidence |
| `objects/` | object_database, never walked (§4.2) |
| `worktrees/`, `modules/`, `lfs/` | linked_worktrees, nested_repositories_and_submodules, lfs_and_external_payloads: `unresolved` with `DependencyUnresolved` if non-empty, else `empty` |
| `shallow`, `info/grafts` (inside the config class) | the owning class becomes `unresolved` with `ContentUnresolved` |
| `*.lock` at any depth, `gc.pid` | the owning class (for a top-level entry, the class of its stem name; otherwise the enclosing class) becomes `unresolved` with `WriterUncontrolled` |
| any other name | `git_configuration_and_hooks` becomes `unresolved` with `ContentUnresolved` |

The alternates class is `unresolved` with `DependencyUnresolved` if `objects/info/alternates` contains any non-comment
line, and `empty` otherwise. It is read through the pin, with no-follow.

### 3.2 Worktree (root `source_repository`, when distinct from `source_git_dir`)

The worktree walk uses one selection:

| Entry | Decision |
|---|---|
| root `.git`, when it is a directory | `IncludeEntryOnly`: the connector directory entry, whose contents belong to §3.1 |
| root `.git`, when it is a gitfile | `Include`, as ordinary bytes. Its `gitdir:` target must be the capability's pinned `source_git_dir`, otherwise `linked_worktrees` becomes `unresolved` with `DependencyUnresolved` |
| root `target`, when the root has regular files `Cargo.toml` **and** `Cargo.lock` | `Skip` (policy-excluded, §3.3) |
| root `.gitmodules` | `Park`: `nested_repositories_and_submodules` becomes `unresolved` with `DependencyUnresolved` |
| a `.git` entry below the root, as a directory or a file | `Park`, the same as `.gitmodules` |
| everything else | `Include` |

`decide` is a pure function of `(path, stat)`, as the 2B2b2a contract requires.

**LFS attributes.** A separate walk of the worktree emits only directories and entries named `.gitattributes`,
skipping everything else, into an in-memory frame sink bounded at 1 MiB. Over the bound, `worktree` becomes
`unresolved` with `ContentUnresolved`. The plan decodes that frame with the 2B2b1 decoder and scans each
`.gitattributes`, together with the git directory's `info/attributes`, for the byte sequence `filter=lfs`. Any match
makes `lfs_and_external_payloads` `unresolved` with `DependencyUnresolved`.

### 3.3 `cargo-target-v1` records

When the policy applies, the plan returns:
- the row `reproducible_outputs = excluded_reproducible`, with `exclusion_id = "cargo-target-v1"`;
- `CustodyExclusionV1 { exclusion_id: "cargo-target-v1", content_class: <the manifest's wire name for
  reproducible_outputs>, policy_version: "cargo-target-v1", reconstruction_dependency_ids: ["cargo-toml",
  "cargo-lock"] }`, plus `"rust-toolchain"` when a root `rust-toolchain.toml` exists;
- one `CustodyDependencyV1 { dependency_id, kind: "worktree-file", binding_digest: SHA-256 of the file's bytes,
  state: captured, reasons: [] }` for each of those files.

The dependency files are also captured in the worktree frame. The exact `content_class` string is whatever
`CustodyManifestV1` validation requires; the implementor confirms it from `custody_seal.rs`.

## 4. The coverage plan (`custody_coverage.rs`, new, `#[cfg(unix)]`)

### 4.1 API

`plan_coverage_v1(request) -> CustodyCoveragePlanV1`.

- **Input:** the capability's pinned `source_repository` and `source_git_dir`, the generation id, the frame
  budgets, the external-evidence declaration (§4.4), the manifest's object inventory (which only decides the
  object-database row), and a runner route for the §2 probe.
- **Output:**
  - canonical `Vec<CustodyCoverageEntryV1>` with all 14 classes;
  - `Vec<CustodyExclusionV1>` and `Vec<CustodyDependencyV1>`;
  - one `CustodyClassReceiptV1 { class, frame_length, frame_sha256, inventory_digest }` per **captured, walked**
    class.

### 4.2 Execution

- Each class walk is a separate `walk_tree_v1` call with its own freshly pinned root. Walks never share a
  `PinnedDirectoryV1` concurrently; they run sequentially (2B2b2a deferral).
- Receipts come from walks into a counting, hashing sink that stores nothing.
- **`object_database` (#7):** never walked, never receipted. It is `captured` if the manifest inventory is non-empty,
  else `empty`.
- The §2 probe writes only inside a planning scratch root. The planning scratch root is a separate, caller-supplied,
  preflighted, owner-private empty directory, with the same overlap preflight as the 2B2 export scratch. It is left
  for the caller to remove.

### 4.3 State derivation and accounting

| Situation | State | Reason |
|---|---|---|
| walked class with at least one entry | `captured` | none |
| walked class with no entries | `empty` | none |
| `reproducible_outputs` under the policy | `excluded_reproducible` | none (the exclusion id) |
| a walker or frame refusal in a class (special file, special mode, path over 4096 bytes, budget, unknown entry, `shallow`, grafts) | `unresolved` | `ContentUnresolved` |
| lock files, `gc.pid` | `unresolved` | `WriterUncontrolled` |
| dependency evidence (§2, §3) | `unresolved` for the evidenced class | `DependencyUnresolved` |
| a device boundary | `unresolved` | `MountBoundary` |
| drift | `unresolved` | `IdentityChanged` |

**Accounting.** For each walk, `skipped_entries` must equal the number of skipped entries that the other classes'
tables own, plus the policy-excluded `target`. A mismatch is a planning bug and refuses the whole plan.

### 4.4 External evidence

A required input with two values, `NoExternalEvidenceRecordedV1` (giving `empty`) and `ExternalEvidenceUnresolvedV1`
(giving `unresolved` with `DependencyUnresolved`). Both are constructible only inside `bridge-core`, and there is no
default.

## 5. Capability binding (fix of #1)

- **Stored in the capability:** the plan's coverage rows, exclusions, dependencies, and receipts. The `Walked`
  stream form is `{ class, generation_id, length, sha256 }`, taken from the receipts; the fixture form is unchanged.
- **Before any scratch write,** the exporter requires exact equality of **all three collections** (coverage,
  exclusions, dependencies) with the manifest's. This adds read-only canonical `exclusions()` and `dependencies()`
  accessors to `CustodyManifestV1` in `custody_seal.rs` (owned).
- **Receipts** must map one-to-one onto the manifest's `captured` non-object classes.
- **The plan's generation** must equal the capability's.
- **A plan containing any `unresolved` row cannot mint a capability.** 2B1 layout would refuse it anyway.

## 6. Staged-frame export (from combined revision 1 §5)

For each captured `Walked` class, in layout order, the exporter:
1. reserves the receipt's frame length plus one entry allowance in the 2B2 ledger;
2. creates `work/payload-<class-code>.frame` through the retained `work/` descriptor;
3. runs `capability.recheck()`;
4. re-walks the class (a fresh pin, the same selection, sequentially) into a frame encoder whose sink is that file;
5. requires the `(frame_length, frame_sha256, inventory_digest)` to equal the receipt, else `SourceDrift`, with no
   seal and a typed incomplete outcome;
6. rewinds the retained descriptor and seals through the existing `PlaintextReaderV1::File` path, never reopening
   by name.

Staged frames count toward the 10 GiB scratch ceiling, and are removed with `work/`.

## 7. Acceptance criteria

Each item needs a dedicated control over real repositories built with system Git in test temp roots.

1. **Classification.** Every §3.1 row routes as specified, including `refs/stash` → `refs_and_head` and `logs/refs/stash`
   → `stash_and_reflogs`. An unknown top-level name, a top-level `*.lock`, a nested `refs/heads/x.lock`, `gc.pid`,
   `shallow`, and `info/grafts` each produce the stated state and reason.
2. **Completeness multiset.** For a rich non-bare clone, the union of every decoded class frame, keyed by
   `(RootDomain, lossless path)`, is checked **as a multiset**. Together with `objects/**` (git directory) and the
   excluded `target/**` (repository), it equals the full source walk, with every key appearing exactly once.
   - The clone has a dirty worktree, untracked and ignored files, a stash, reflogs, an in-progress merge, hooks,
     `info/sparse-checkout`, and bridge evidence.
   - A worktree file named `HEAD` and the git directory's `HEAD` are distinct keys.
3. **Connector.** The root `.git` directory appears once, as a childless directory entry in the worktree frame.
   A gitfile checkout (`--separate-git-dir`) captures the gitfile bytes, and passes only when its target is the
   pinned git directory.
4. **`cargo-target-v1`.**
   - With `Cargo.toml` and `Cargo.lock`, the root `target` is excluded, the exclusion and dependency records
     validate in a manifest, and changing `Cargo.lock` changes the dependency digest.
   - Without either file, `target` is captured.
   - A nested `target` is always captured.
5. **Dependency evidence.** Each of the following parks the class it names, even with no `.git` storage present:
   - an index gitlink (`update-index --cacheinfo 160000`) with no `.gitmodules`;
   - a `.gitmodules`;
   - a nested `.git`;
   - `filter=lfs` in a nested `.gitattributes`, and in `info/attributes`;
   - non-empty `worktrees/`, `modules/`, and `lfs/`;
   - an alternates line.
6. **Binding (#1).** A plan and manifest differing only in one coverage row, in either direction, or only in one
   dependency digest, refuse before any scratch write (proved by a scratch snapshot).
7. **Object database (#7).** An empty initialized repository gives `empty`, and a one-object repository gives
   `captured`. Neither requests a frame.
8. **Drift.** A file edited between plan and export refuses with `SourceDrift`, with no seal and a typed incomplete
   outcome.
9. **Round trip.** Every sealed coverage payload decodes, with the 2B2b1 decoder, to exactly that class's walked
   entries.
10. **Probe.** The §2.4 runner controls pass. The probe never writes outside the planning scratch, and the source's
    `index` bytes and mtime are unchanged.
11. **Sequential walks.** A seam proves that no two walks hold the same pin concurrently.
12. **Regressions.** Every 2B2, 2B2b1, and 2B2b2a control still passes.

## 8. RED-first and mutation evidence

As in the earlier children:
- a structural RED;
- a behavioral RED per control;
- a persisted, foreground mutation matrix with one row per guard, at minimum:
  - each table row mis-routed;
  - the unknown-name park off;
  - the lock park off;
  - the connector descending, or omitted;
  - the policy preconditions off;
  - each dependency-evidence detector off;
  - the gitlink parser accepting malformed input or ignoring `160000`;
  - the skip accounting off;
  - the binding equality off for each collection;
  - the receipt comparison off;
  - the object-database row walked;
  - the staged frame reopened by name;
- byte-exact restores, and a snapshot proof after the matrix.

## 9. Verification

The parent §8 gates, plus the platform lanes:
- the container (Linux, overlayfs);
- native ext4 in CI;
- the controller's macOS lane.

Use `--no-fail-fast`, unset the proxy variables for workspace runs, and record exact totals and exclusions.

## 10. Files and stop conditions

**Owned paths:**
- `custody_git.rs` and `custody_git_tests.rs`: only the §2 command, its parser if it lives there, and its controls;
- `custody_coverage.rs` and `custody_coverage_tests.rs` (new);
- `custody_export.rs` and `custody_export_tests.rs`: §5 and §6 only;
- `custody_seal.rs`: the two read-only accessors only;
- `lib.rs`: the module declaration;
- the handoff `docs/superpowers/reviews/<date>-adr0041-slice2b2b2b-implementation-handoff.md`.

**Stop conditions:** stop and report if any of the following is needed:
- a change to `custody_frame.rs`, `custody_walk.rs`, or `fs_custody.rs`;
- a new manifest field or reason code;
- following a symlink;
- a path-addressed source read;
- a Git command other than `ls-files --stage -z`;
- a new dependency.

Also stop if the review finds an open-class population, or if the diff clearly exceeds one reviewable child.

## 11. Review

The spec and implementation reviews each have a two-round cap, run by Sol/xhigh in hard read-only mode. Findings are
tagged WRONG or SMELL and MATERIAL or IMMATERIAL to §1.1. Implementation and repairs are by Opus 5.5 through
`a2a-bridge implement`, and the PR merges on approval and green CI.

## 12. Commit message

feat(bridge-core): ADR-0041 Slice 2B2b2b class coverage, manifest binding, gitlink probe, and staged-frame export
