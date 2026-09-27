---
task-type: implement
---
# Implement ADR-0041 Slice 2B2b2b1: the coverage plan (class tables, evidence detection, gitlink probe, policy records)

**Revision:** 4 (folds the extension round, see §12). Revision 1 was the combined 2B2b2b task (`47d89436`). Sol spec review round 1 rejected it with 9
closed blockers, and the owner approved splitting it into 2B2b2b1 (this plan) and 2B2b2b2 (binding and export) on
2026-09-27. See §12.
**Implementation base:** current `main`; bind the exact SHA at dispatch. The predecessor is 2B2b2a, PR #117 at
`f42c81fc`.
**Parent plan:** `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md`. The serial order is
2B2b2a → **2B2b2b1** → 2B2b2b2 → 2B3.
**Design notes:** `docs/superpowers/plans/2026-09-26-adr0041-slice2b2b2b-coverage-design-notes.md`.

## 1. Description

### 1.1 Goal

A read-only planner turns a pinned, quiescent clone into a **coverage plan**:
- canonical coverage rows for all 14 classes;
- the `cargo-target-v1` exclusion and dependency records;
- one frame receipt per captured, walked class.

In the plan:
- every git-directory and worktree entry is owned by exactly one class, recorded as excluded-reproducible under
  `cargo-target-v1`, or makes its owning class `unresolved`;
- dependency evidence (gitlinks, `.gitmodules`, nested repositories, LFS attributes, linked worktrees, alternates,
  locks) is detected and never hidden;
- the source is never written, and the planner writes only inside a bounded planning scratch.

2B2b2b2 later binds this plan to the manifest and exports it. This child never touches the exporter.

### 1.2 Owner decisions in force

- the common-clone set, and capture of the whole worktree except `cargo-target-v1` (2026-09-26);
- exact gitlink detection through one read-only runner command, `ls-files --stage -z` (2026-09-26);
- the plan/export split (2026-09-27);
- a hostile same-user racer is out of scope: detect and refuse.

### 1.3 Non-scope

This child does not include capability binding, staged frames, exporter changes, or `custody_seal.rs` accessors
(all in 2B2b2b2). It also excludes production quiescence and minting, restore, and wiring.

## 2. Runner amendment and the index probe

### 2.1 Command

Add `GitCommandV1::LsFilesStageZ`, with the exact argv `ls-files --stage -z`, to `custody_git.rs`:
- `permits_object_store_route` is **false** for it;
- it runs under the unchanged closed environment, rooted spawn, bounded I/O, and deadline;
- it adds no unsafe code, so 2B2a's A14 inventory is unchanged.

### 2.2 Planning scratch and probe (fixes #1 and #8)

- **Scratch.** The planner receives a caller-supplied **planning scratch root**: preflighted, owner-private, and
  empty, and a caller budget `CustodyPlanningBudgetV1 { max_scratch_bytes }`.
- **Overlap preflight (fix of round 2 #2).** Before any scratch write, the planner refuses a scratch root that
  overlaps the **full protected-directory set**:
  - the source repository;
  - the source git directory;
  - the primary object store;
  - every store of the pinned, recursive alternate chain.

  It reuses the 2B2 exporter's retained-identity overlap predicate (`refuse_source_overlap` over
  `protected_directories`) and its `pin_alternate_chain`. A refusal is `InvalidScratch`, with no entry created.
- **Pre-write drift barrier (fix of extension round #1).** Immediately after the overlap preflight, and **before the
  first ledger reservation or scratch directory creation**, the planner rechecks the whole protected set:
  - every retained descriptor's identity;
  - each object store's `objects/info/alternates` content digest.

  This is exactly the exporter's `PinnedObjectStoreV1::recheck` semantics, which 2B2's `capability.recheck()` runs
  at the same point. Any change is `SourceRootDrift`, and no scratch entry exists. It is required because an
  alternates rewrite between pinning and the first write could newly reference a store that contains the scratch
  root.
- **Layout.** It creates `work/`, `work/home/`, and `work/xdg/`, and roots the runner at `work/` exactly as 2B2 does.
- **Probe repository.** It creates the probe with `InitBare { dir: "index-probe.git", object_format: <the source
  object format> }`, a single-component operand. `GIT_DIR` is `index-probe.git`.
- **Copy.** It copies the source `index`, and every `sharedindex.*`, read through the pinned `source_git_dir` with
  `open_regular_file`, into the probe with `create_new_regular_child`.
- **Ledger (fix of round 2 #1).** One planning ledger, reusing 2B2's `ScratchLedgerV1` **and its init-reservation
  constants and helpers** (`INIT_BARE_ENTRIES_V1`, the logical `HEAD`/`config` bound, and `INIT_BARE_FILES_V1`),
  shared rather than duplicated, charges:
  - `work/`, `home/`, and `xdg/` (entries);
  - the probe initialization (reserved before the spawn, then remeasured and reconciled);
  - every copied file (its bytes plus an entry allowance, reserved before the copy).

  Over budget is `CustodyCoverageErrorV1::PlanningScratchBudget`, which refuses the whole plan (§4.5).
- **No index.** A missing source `index` means no probe runs, and the result is `NoIndex`, which is `empty` gitlink
  evidence.
- **Probe P1** (controller, Git 2.54.0): a copied index in a template-less bare probe lists as
  `<mode> <oid> <stage>\t<path>\0` with exit status 0. A gitlink appears as `160000`, and nothing is written.

### 2.3 Status and bounds (fix #7)

- **Exit status.** It must be success, checked with the same `require_success` discipline as 2B2, before any
  parsing. A nonzero status or a timeout makes `index` `unresolved` with `ContentUnresolved`. This covers, for
  example, a split index whose `sharedindex.*` is missing.
- **stdout limit:** `min(128 × copied index bytes + 64 KiB, 256 MiB)`. Exceeding it makes `index` `unresolved` with
  `ContentUnresolved`.

### 2.4 Parser

The output is strict records, each `^[0-7]{6} <hex oid of the source format length> [0-3]\t<path>\0`, where every
path satisfies the 2B2b1 path policy.
- A malformed record, trailing bytes, or an overlong path makes `index` `unresolved` with `ContentUnresolved`.
- Any `160000` record makes `nested_repositories_and_submodules` `unresolved` with `DependencyUnresolved`.

## 3. Ownership and detection

### 3.1 Git directory (root `source_git_dir`): whole-subtree ownership

Each top-level entry is routed by this closed table. Each class walk (§4) **Includes** its own top-level entries,
whole subtrees, and **Skips** the rest.

| Top-level entry (exact, or `*` prefix) | Class |
|---|---|
| `HEAD`, `ORIG_HEAD`, `FETCH_HEAD`, `packed-refs`, `refs/` (all, including `refs/stash`) | refs_and_head |
| `logs/` (all) | stash_and_reflogs |
| `index`, `sharedindex.*` | index |
| `config`, `config.worktree`, `description`, `hooks/`, `info/` (all), `branches/`, `remotes/`, `rr-cache/`, `gc.log` | git_configuration_and_hooks |
| `MERGE_HEAD`, `MERGE_MSG`, `MERGE_MODE`, `MERGE_RR`, `MERGE_AUTOSTASH`, `AUTO_MERGE`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `REBASE_HEAD`, `SQUASH_MSG`, `COMMIT_EDITMSG`, `TAG_EDITMSG`, `BISECT_*`, `NOTES_MERGE_REF`, `NOTES_MERGE_PARTIAL/`, `NOTES_MERGE_WORKTREE/`, `rebase-merge/`, `rebase-apply/`, `sequencer/` | in_progress_git_operations |
| `a2a-bridge/`, `A2A_*` | bridge_evidence |
| `objects/` | object_database: never framed; the object sentinel scan is in §3.3 |
| `worktrees/`, `modules/`, `lfs/` | linked_worktrees, nested_repositories_and_submodules, lfs_and_external_payloads: **unresolved** with `DependencyUnresolved` if non-empty, else `empty` |
| `commondir` | linked_worktrees becomes **unresolved** with `DependencyUnresolved` (the source is itself a linked-worktree git directory) |
| `shallow` | refs_and_head becomes **unresolved** with `ContentUnresolved` |
| `info/grafts` (inside the config class) | git_configuration_and_hooks becomes **unresolved** with `ContentUnresolved` |
| `*.lock` or `gc.pid` at the top level | the class of the stem name (for example `index.lock` belongs to index), or git_configuration_and_hooks when there is no stem match, becomes **unresolved** with `WriterUncontrolled` |
| `*.lock` below the top level, in a walked class | the enclosing class becomes **unresolved** with `WriterUncontrolled` |
| any other name | git_configuration_and_hooks becomes **unresolved** with `ContentUnresolved` |

Fix #9 added `MERGE_RR`, and an audit against system Git states added `TAG_EDITMSG`, `NOTES_MERGE_*`, and
`commondir`. A test builds real repositories for rerere, notes-merge, and linked-worktree states.

Alternates: `objects/info/alternates`, read through the pin with no-follow. Any non-comment line makes
`alternates_and_shared_stores` **unresolved** with `DependencyUnresolved`; otherwise it is `empty`.

### 3.2 Worktree (root `source_repository`, when distinct from `source_git_dir`)

| Entry | Decision |
|---|---|
| any directory whose `(dev, ino)` equals the pinned `source_git_dir` identity, at **any** depth: the root `.git`, or an in-worktree `--separate-git-dir` (fix #5) | `IncludeEntryOnly`: the connector entry, whose contents belong to §3.1 |
| root `.git`, when it is a gitfile | `Include`, as bytes. Its target must resolve to the pinned git directory (by identity); otherwise linked_worktrees becomes **unresolved** with `DependencyUnresolved` |
| root `target`, when it is a **directory** (SMELL-1) and the root has regular files `Cargo.toml` and `Cargo.lock` | `Skip`: policy-excluded (§3.4) |
| root `.gitmodules` | `Park`: nested_repositories_and_submodules becomes **unresolved** with `DependencyUnresolved` |
| a `.git` entry below the root, other than the pinned git directory | `Park`, the same as above |
| everything else, including a root `target` that is a regular file or a symlink | `Include` |

`decide` depends only on `(path, stat)` and constants fixed at plan start (the pinned identities and the policy
precondition), so it satisfies the 2B2b2a purity contract.

### 3.3 Detector walks (fixes #2, #3, and #6)

Detector walks are separate `walk_tree_v1` calls into an in-memory sink. They are **exempt from the class-skip
equation (§4.3)**: their completeness comes from pruning exactly what the class walks prune.

- **LFS attributes.**
  - **Pruning:** the walk prunes the pinned-git-directory connector and every nested `.git` (`Skip`), and the
    policy-excluded root `target` (`Skip`).
  - **Selection:** it `Include`s every other directory and every entry named `.gitattributes`, and `Skip`s all other
    files.
  - **Budget:** the sink is bounded at 1 MiB and the walk at the plan's entry budget. Exceeding either makes
    `worktree` **unresolved** with `ContentUnresolved`.
  - **Scan:** each decoded `.gitattributes`, together with `info/attributes` (read directly through the pin, never by
    walking `.git`), is scanned for the byte sequence `filter=lfs`. A match makes `lfs_and_external_payloads`
    **unresolved** with `DependencyUnresolved`.
  - **Outcome:** an excluded `target/**/.gitattributes` is never read and never consumes budget.
- **Object lock sentinel.**
  - **Selection:** a walk of `objects/` `Include`s directories, `Park`s on any file named `*.lock` (which makes
    `object_database` **unresolved** with `WriterUncontrolled`), and `Skip`s every other file. Object contents are
    never read.
  - **Budget:** exceeding the entry budget makes `object_database` **unresolved** with `ContentUnresolved`.

### 3.4 `cargo-target-v1` records

When the policy applies, the plan returns:
- the row `reproducible_outputs = excluded_reproducible` with `exclusion_id = "cargo-target-v1"`;
- `CustodyExclusionV1 { exclusion_id: "cargo-target-v1", content_class, policy_version: "cargo-target-v1",
  reconstruction_dependency_ids }`;
- one `CustodyDependencyV1 { dependency_id, kind: "worktree-file", binding_digest: <SHA-256 of the file>, state:
  captured, reasons: [] }` for each dependency file.

Details:
- `content_class` is exactly `"reproducible_outputs"`, the snake-case wire name of
  `CustodyCoverageClassV1::ReproducibleOutputs` (fix of round 2 #3). Manifest validation requires only a non-empty
  string, so the spec fixes the value. A test asserts the planner's exact emitted record, and a mutation of that one
  value fails it.
- The dependency ids are `cargo-toml` and `cargo-lock`, plus `rust-toolchain` when a root regular file
  `rust-toolchain.toml` exists. The files are also captured in the worktree frame.

## 4. The plan

### 4.1 API

```text
plan_coverage_v1(request) -> Result<CustodyCoveragePlanV1, CustodyCoverageErrorV1>
```

The request contains:
- the pinned source repository and git directory, the primary object store, and the pinned alternate chain, which
  together are the full protected set (§2.2);
- the generation id;
- the frame and entry budgets;
- `CustodyPlanningBudgetV1`;
- the planning scratch root;
- the external-evidence declaration (§4.4);
- the source object format;
- the manifest object inventory, used only for the object-database row;
- the runner route.

The plan contains:
- canonical coverage rows (all 14 classes);
- exclusions and dependencies;
- one `CustodyClassReceiptV1 { class, frame_length, frame_sha256, inventory_digest }` per captured, walked class.

### 4.2 Execution

- **Sequential walks.** Walks run strictly one at a time, each with its own freshly pinned root, and never share a
  `PinnedDirectoryV1` concurrently (the 2B2b2a deferral). A seam proves this.
- **Receipts.** Class walks go into a counting, hashing sink that stores nothing. The receipts are the frame summary
  plus the walker's inventory digest.
- **`object_database`.** It is never framed. It is `captured` if the inventory is non-empty, else `empty`, unless the
  §3.3 sentinel makes it `unresolved`.

### 4.3 States and class-skip accounting (fix #2 scope)

| Situation | State | Reason |
|---|---|---|
| walked class with at least one entry | `captured` | none |
| walked class with no entries | `empty` | none |
| `reproducible_outputs` under the policy | `excluded_reproducible` | none |
| a class-local walker or frame refusal (special file, special mode, path over 4096 bytes, budget) | `unresolved` | `ContentUnresolved` |
| a lock or `gc.pid` | `unresolved` | `WriterUncontrolled` |
| dependency evidence | `unresolved` (the evidenced class) | `DependencyUnresolved` |
| a device boundary | `unresolved` | `MountBoundary` |
| drift | `unresolved` | `IdentityChanged` |

**Class-skip equation** (capture walks only). For each class walk, `skipped_entries` must equal the entries that
the other classes' tables own at the top level, plus the policy-excluded `target` for the worktree walk. A mismatch
is `CustodyCoverageErrorV1::AccountingMismatch`, which refuses the plan.

### 4.4 External evidence

A required input: `NoExternalEvidenceRecordedV1` gives `empty`; `ExternalEvidenceUnresolvedV1` gives `unresolved`
with `DependencyUnresolved`. Both are constructible only inside `bridge-core`, and there is no default.

### 4.5 Errors (SMELL-2)

`CustodyCoverageErrorV1` covers failures that make **no** plan trustworthy:
- `InvalidScratch` (preflight or overlap);
- `PlanningScratchBudget`;
- `AccountingMismatch`;
- `SourceRootDrift` (a pinned source root changed);
- `Io`, when it occurs on a pinned root itself.

Everything class-local becomes an `unresolved` row, never an error:
- an unreadable entry;
- a probe timeout or nonzero exit;
- walker refusals.

**Runner failure mapping** (round 2 SMELL, made exhaustive and deterministic in revision 4). Every runner or stage
outcome maps to exactly one result:

| Stage | Outcome | Result |
|---|---|---|
| overlap or preflight | overlap, invalid scratch | `InvalidScratch` (plan refused) |
| pre-write barrier | identity or alternates digest change | `SourceRootDrift` (plan refused) |
| any stage | ledger reservation over budget, or remeasure over reservation | `PlanningScratchBudget` (plan refused) |
| any runner call | `InvalidRoute`, `InvalidCommand`, `InvalidObjectStoreRoute`, `ObjectStoreRouteRefused`, `RouteRefusal`, `RouteIdentityChanged`, `DigestMismatch`, `BinaryDrift`, `UnsupportedVersion`, `Spawn`, `Fs`, `StdinNotRegular`, `StdinLimit`, `Stdin` | `ProbeInfrastructure` (plan refused) |
| `InitBare` | nonzero exit, `Timeout`, `Stream`, `StdoutLimit`, `StderrLimit` | `ProbeInfrastructure` (plan refused) |
| index copy | an open or read failure of the source `index` or `sharedindex.*` through the pin | `index` becomes `unresolved` with `ContentUnresolved` |
| `LsFilesStageZ` | nonzero exit, `Timeout`, `Stream`, `StdoutLimit`, `StderrLimit` | `index` becomes `unresolved` with `ContentUnresolved` |
| `LsFilesStageZ` parse | a malformed record, trailing bytes, an overlong path | `index` becomes `unresolved` with `ContentUnresolved` |
| no source `index` | | no probe, and `NoIndex` evidence |

`CustodyCoverageErrorV1` therefore has exactly these variants: `InvalidScratch`, `SourceRootDrift`,
`PlanningScratchBudget`, `ProbeInfrastructure`, `AccountingMismatch`, and `Io` (on a pinned root). **One control
and one mutation per distinct cell** of this table. A shared runner seam injects each `CustodyGitError` variant.

A test covers each error variant, and one class-local case per reason code.

## 5. Acceptance criteria

1. **Probe.**
   - SHA-1 and SHA-256 repositories with an index reach one successful exact-argv probe (a nested operand fails
     before spawn).
   - A gitlink with no `.gitmodules` parks nested.
   - A missing `sharedindex`, and a seam returning exit 1 with empty stdout, each make `index` unresolved.
   - Stdout max and max+1 give success and unresolved.
   - Each malformed parser shape is refused.
   - The source `index` bytes and mtime are unchanged.
2. **Planning ledger and overlap.**
   - Exact max and max+1 are tested, including the init overhead and copied bytes. The ledger equals an independent
     census of the planning scratch, and removing any charge flips the test.
   - A scratch root that is an empty descendant of an alternate store, and one that is an identity alias of an
     alternate store, each refuse `InvalidScratch` with no entry created.
   - After request construction, rewriting the primary alternates file to name a store that contains the scratch root
     refuses `SourceRootDrift` before any scratch entry appears. An unchanged chain succeeds.
3. **Class table.** Every §3.1 row routes as specified, and every special row (`commondir`, `shallow`, grafts,
   locks, the unknown name) gives its state and reason. Real rerere (`MERGE_RR`), notes-merge, and bisect states are
   captured in `in_progress_git_operations`.
4. **Completeness multiset.** For a rich non-bare clone, receipts are re-walked into decodable frames by a test
   helper. The union of class frames, keyed by `(RootDomain, path)`, plus `objects/**` and the excluded `target/**`,
   equals the full source walk, with every key appearing exactly once.
   - The clone has a dirty worktree, untracked and ignored files, a stash, reflogs, an in-progress merge with
     rerere, hooks, `info/sparse-checkout`, and bridge evidence.
   - It includes a worktree file named `HEAD`.
5. **In-worktree separate git directory (#5).** `--separate-git-dir=repo/.gd` makes `.gd` a childless connector in
   the worktree frame. Each physical entry has exactly one owner.
6. **Detectors (#3, #6).**
   - A large excluded `target/` containing `filter=lfs` neither consumes the detector budget nor parks LFS.
   - A nested `.gitattributes` with `filter=lfs`, and `info/attributes` with `filter=lfs`, each park LFS.
   - `objects/info/commit-graph.lock` parks `object_database` with `WriterUncontrolled`.
7. **Skip accounting (#2).** An ordinary clone with a `README` and no attributes plans successfully. Applying the
   class equation to a detector receipt would fail it (a negative control).
8. **`cargo-target-v1`.**
   - With `Cargo.toml` and `Cargo.lock`, the records validate when assembled into a manifest in the test, and
     changing `Cargo.lock` changes the digest.
   - A regular-file root `target`, and a symlink root `target`, are captured (SMELL-1).
   - A nested `target` is captured.
9. **Object database.** An empty initialized repository gives `empty`, and a one-object repository gives
   `captured`. No frame is requested either way.
10. **Errors.** Every §4.5 variant, and one class-local case per reason, is tested.
11. **Sequential walks.** A seam proves no two walks hold the same pin.
12. **Regressions.** Every 2B2, 2B2a, 2B2b1, and 2B2b2a control still passes.

## 6. RED-first and mutation evidence

As before:
- a structural RED;
- a behavioral RED per control;
- a persisted, foreground mutation matrix with one row per guard, at minimum:
  - each table row mis-routed;
  - each special row off;
  - the probe exit check off;
  - the probe ledger charges off;
  - the parser leniencies;
  - `160000` ignored;
  - the connector descending, or keyed by path instead of identity;
  - the target kind check off;
  - each detector's pruning off;
  - the object sentinel off;
  - the class-skip equation off, or applied to detectors;
  - the error-versus-row mapping swapped;
  - walks made concurrent;
- byte-exact restores, and a snapshot proof after the matrix.

## 7. Verification

The parent plan §8 gates, run with `--no-fail-fast`. Platform lanes:
- the container (Linux, overlayfs; unset the proxy variables for workspace runs);
- native ext4 in CI;
- the controller's macOS lane.

`cargo deny` runs where installed; otherwise it is a named exclusion and CI runs it.

## 8. Files

- `crates/bridge-core/src/custody_git.rs` and `custody_git_tests.rs`: the §2.1 command, and its controls only;
- `crates/bridge-core/src/custody_coverage.rs` and `custody_coverage_tests.rs` (new; `#[cfg(unix)]`);
- `crates/bridge-core/src/lib.rs`: the module declaration only;
- `crates/bridge-core/src/custody_export.rs`: **visibility only, or a behavior-preserving extraction**:
  - `pub(crate)` on `ScratchLedgerV1` and the ledger methods the planner needs;
  - `pub(crate)` on the init-reservation constants and helper;
  - `pub(crate)` on `pin_alternate_chain`, `PinnedObjectStoreV1` and its `recheck`, `refuse_source_overlap`, and the
    protected-set helper;
  - or a move of those items into a neutral shared module.

  No behavior change; every 2B2 control still passes.
- `docs/superpowers/reviews/<date>-adr0041-slice2b2b2b1-implementation-handoff.md`.

## 9. Stop conditions

Stop and report if any of the following is needed:
- a change to `custody_frame.rs`, `custody_walk.rs`, `fs_custody.rs`, or `custody_seal.rs`, or any behavioral change
  to `custody_export.rs` (visibility per §8 is allowed);
- a new manifest field or reason code;
- following a symlink;
- a path-addressed source read;
- any Git command other than `ls-files --stage -z` and the existing `InitBare`;
- a new dependency.

Also stop on an open-class review population, or a review cap exhausted without convergence.

## 10. Review

The spec and implementation reviews each have a two-round cap. This revision is spec round 2 of the 2B2b2b lineage,
under the budget-inheritance rule. Findings are tagged WRONG or SMELL and MATERIAL or IMMATERIAL to §1.1.
Implementation and repairs are by Opus 5.5 through `a2a-bridge implement`, and the PR merges on approval and green
CI.

## 11. Commit Message

feat(bridge-core): ADR-0041 Slice 2B2b2b1 coverage plan, evidence detection, and gitlink probe

## 12. Review history and carried-item ledger

**Round 1**, on combined revision 1 (`47d89436`), gave REJECT, raw result SHA-256 `197fdddf…`. It reported 9 WRONG
MATERIAL blockers and 4 SMELLs. The owner approved the plan/export split.

| Item | Status | Where |
|---|---|---|
| #1 `InitBare` operand and object format | RESOLVED | §2.2 |
| #2 skip equation vs detector walk | RESOLVED | §3.3, §4.3 |
| #3 detector descends into excluded domains | RESOLVED | §3.3 |
| #4 drifted re-walk exceeds reservation | moved to 2B2b2b2 (bounded staged writer) | design notes |
| #5 in-worktree separate git directory captured twice | RESOLVED | §3.2 (identity-keyed connector) |
| #6 locks under `objects/` unseen | RESOLVED | §3.3 object lock sentinel |
| #7 probe exit status | RESOLVED | §2.3 |
| #8 planning scratch budget, and init not charged | RESOLVED | §2.2 |
| #9 `MERGE_RR` missing | RESOLVED | §3.1, and an audit adding `TAG_EDITMSG`, `NOTES_MERGE_*`, and `commondir` |
| SMELL-1 `target` must be a directory | RESOLVED | §3.2 |
| SMELL-2 planner failure semantics | RESOLVED | §4.5 |
| SMELL-3 status ledger | RESOLVED | this table |
| SMELL-4 size | RESOLVED | owner-approved split |
| Design note #1 (three-collection binding) | moved to 2B2b2b2 | design notes |
| Design notes #2, #3, #6, #7, and the multiset | RESOLVED here | §3, §4, §5.4 |
| Design note #4 (policy records) | RESOLVED | §3.4 |
| The 2B2b2a shared-pin deferral | RESOLVED | §4.2 |

**Round 2** (final admitted, on revision 2 at `5ae95a90`): REJECT. Every round-1 item was RESOLVED, or correctly
MOVED to 2B2b2b2. It raised three new closed blockers, all folded in revision 3:
- **#1 (ledger reachability):** an owned, visibility-only change to `custody_export.rs` (§8).
- **#2 (alternate stores missing from the overlap preflight):** the full protected set, and the reused 2B2
  predicate (§2.2).
- **#3 (`content_class`):** the literal `"reproducible_outputs"`, with an exact-record test (§3.4).

Its SMELL (runner-failure mapping) is folded as the §4.5 table. Findings went from 9 to 3, none repeating, so
revision 3 is reviewed in one disclosed extension round under the owner's authorization while converging.

**Extension round** (on revision 3 at `39f7932d`): REJECT. Round-2 #1 and #3 were RESOLVED; #2 was UNRESOLVED,
because a pre-write alternate-chain drift barrier was missing. Revision 4 folds it, plus both SMELLs:
- the pre-write protected-set recheck (§2.2), with a red regression (§5.2);
- an exhaustive, deterministic runner-failure table, with one control and mutation per cell (§4.5);
- shared init-reservation constants (§2.2, §8).

Findings went 9 → 3 → 1, so the loop is still converging. Revision 4 gets one further narrow extension round,
disclosed.
