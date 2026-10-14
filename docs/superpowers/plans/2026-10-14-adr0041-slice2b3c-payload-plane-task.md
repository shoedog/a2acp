---
task-type: implement
---
# ADR-0041 Slice 2B3c — payload plane (task specification)

**Revision:** 1 (draft for Sol/xhigh spec review; cap two rounds, declared before dispatch).
**Drafting base:** `5e3fec9367300bc12c37840f9d4ccaeebad2b04f` (main after PR #138). 2B3b merged in PR #137 at
`4fcda06d`.
**Implementor:** Opus 5.5 through `a2a-bridge implement` (the owner's standing implementor direction of
2026-09-24; the design's Sonnet 5.5 selection applied to 2B3b only). Require transcript evidence of
`claude-opus-5-5`.
**Authority:** `docs/superpowers/specs/2026-09-27-adr0041-slice2b3-restore-design.md` §2, §3.3, §4, §5 and
§7; parent `2026-09-20-adr0041-slice2b-local-capsule-plan.md` §6 and §8; ADR-0041 §6 and §9.

## Description

Consume the crate-private phase-G success value `RestoredGitPlaneV1` and materialize the rest of the active
payload and the inert evidence plane into the same destination:

- the **worktree** frame into `repository/` (directories, regular files with their exact modes, symlinks
  written verbatim);
- the **index** frame into `repository/.git/` (`index` and `sharedindex.*`, byte-exact);
- the three **evidence-only** classes into `evidence/<class wire name>/`, where nothing reads or executes them.

Return a crate-private phase-P value that retains everything phase G retained, plus the new pins and the
inventories 2B3d's record needs. This slice writes no `custody-restore.v1` record, adds no joint fixture, and
adds no operator or CLI wiring; those are 2B3d.

**Scope note.** The 2B3b controller handoff says "2B3c payload; 2B3d index, evidence, record". The merged
design (§3.3, §5) is the authority and puts the index and the evidence plane in 2B3c. This spec follows the
design.

## Decisions this spec fixes (the design leaves them open)

| # | Question | Decision |
|---|---|---|
| D1 | `evidence/<class-code>`: numeric code or name? | The class's snake-case wire name: `evidence/git_configuration_and_hooks/`, `evidence/in_progress_git_operations/`, `evidence/bridge_evidence/`. The root gitfile (D3) goes to `evidence/worktree_root_gitfile/gitfile`. |
| D2 | Evidence modes | Evidence is inert. Regular files are created `0400` and directories end `0500`, whatever the frame says. The frame's original mode is kept in the returned evidence inventory (2B3d records it). Evidence symlinks are written verbatim with the new symlink primitive and listed like worktree symlinks; nothing in the restore ever follows them. |
| D3 | The worktree `.git` connector | Coverage emits the pinned git dir as a childless `Directory` entry, and a root gitfile as a `Regular` entry at path `.git`. Phase P skips exactly one childless `Directory` at path `.git` (because `repository/.git` already exists from phase G). A `Regular` at path `.git` is the root gitfile: its bytes go to `evidence/worktree_root_gitfile/gitfile` and nothing is created at `repository/.git`. Any entry *beneath* `.git/`, a `.git` symlink, or a `.git` directory that has children refuses as `PayloadFrameInvalid` before any phase-P write. A childless directory elsewhere (a separate git dir that lived inside the worktree) is restored as an ordinary empty directory; it is inert, and this is disclosed in the result. |
| D4 | Worktree, index and `.git` modes | Worktree regular files and directories get the frame's exact mode (the decoder already limits modes to `0o777`). Directory modes are applied post-order, after the directory's children exist, so a `0555` directory still receives its children. The index and `sharedindex.*` are byte-exact and created `0600`, the phase-G convention for `.git` metadata; their frame modes are kept in the result. |
| D5 | Other Captured classes | Only these classes may carry a payload at phase P: Worktree, Index, InProgressGitOperations, GitConfigurationAndHooks, BridgeEvidence (phase G owns RefsAndHead, ObjectDatabase and StashAndReflogs). If any other class is Captured, phase P refuses at admission with a typed `UnsupportedCapturedClass` and creates nothing. The 2B2 exporter never captures those classes; this is a fail-closed guard, tested with `stage_frame`. |
| D6 | Path spelling on the destination | The worktree keeps real-world names, so phase G's printable-ASCII rule does NOT apply to worktree or evidence paths. Prevalidation still refuses, before any write: `.`/`..`/empty components, a parent that is not a `Directory` entry in the same frame, a path appearing twice, and an exact-byte duplicate. A destination file system that folds case or Unicode normalization may still collide two distinct frame names; create-new then returns "exists" and phase P refuses with a typed `DestinationCollision` after writes began (a `PayloadPlaneRestoreFailureV1` with partial progress). That outcome is acceptable and must be tested on macOS (APFS folds case). |
| D7 | Escaping symlink targets | A symlink target is "external" when it is absolute, or when lexically resolving it against the link's parent directory leaves the restored root (`repository/` for the worktree, the class directory for evidence). Every external target is returned in `declared_external_references` with its path, target bytes and plane. Targets are written verbatim either way. |
| D8 | Cross-phase failure type | Phase G already returns `GitPlaneRestoreFailureV1`; the design's `PartialRestoreV1` does not exist yet. 2B3c defines `PayloadPlaneRestoreFailureV1` for phase P only. 2B3d, which owns the whole restore wrapper, composes the phase failures. |

## Global constraints

- Rust 1.94.0. No new dependencies, runner commands, wire or protocol changes, or public API. Unix-only and
  production-unwired, like 2B3a/2B3b.
- **Inputs.** All payload bytes come from the retained staged descriptors in the phase-V value that phase G
  retained. Never reopen a capsule or staging path. Before decoding a payload, re-verify its descriptor with
  `reverify_staged_v1` (length and SHA-256). Each frame was decoded completely once in phase V; phase P
  prevalidates it completely again into a bounded table (no writes), then decodes it a third time while
  materializing. A decode error in any pass is fatal; no success rests on provisional decoder entries
  (an error at the trailer after materialization began is a failure with partial progress).
- **Writes.** Every parent is a pinned directory this invocation created (or `repository/`, `repository/.git`
  and the destination root, all pinned by phases V/G). Create-new and no-follow only. Never overwrite,
  rename, chmod a pre-existing object, follow a link, or delete. A preexisting entry in an intended slot is a
  collision. Every created directory and file is checked to be on the destination root's device.
- **Primitives (increment C1).** Add to `fs_custody.rs`, beside `create_new_regular_child` and
  `read_child_symlink`:
  - `create_new_symlink_child(name, target: &[u8])`: `symlinkat` relative to the pinned directory, then a
    no-follow `fstatat` that must show a symlink owned by the effective user on the parent's device, whose
    `readlinkat` bytes equal `target`. This is the only new `unsafe` site.
  - Exact file modes use one fixed shape: the existing `create_new_regular_child` (create-new at `0600`),
    write and sync the content, then a new `set_exact_mode(&File, mode)` that `fchmod`s through the retained
    descriptor (`set_permissions`; no `unsafe`; unaffected by umask) and confirms the mode with `fstat`.
    So a file whose final mode lacks owner-write still receives its content first.
  - The same `set_exact_mode` on a pinned directory this invocation created, applied post-order after the
    directory's children exist.
  - Update both existing unsafe-inventory controls (`custody_git_tests.rs`
    `a14_ast_inventory_keeps_new_unsafe_boundaries_exact` and `custody_walk_tests.rs`
    `unsafe_inventory_pins_the_new_2b2b2a_sites_and_their_files`) with exactly the new site, and add a 2B3c
    control in the same style (as `custody_mounts_tests.rs` does) that pins the new function's unsafe count.
- **Budget.** Use the one `ScratchLedgerV1` carried from phase V through phase G. Reserve the entry
  allowance before each create-new and the byte count before each write chunk; release only on a create
  that did not happen. Never create a second ledger or replenish it. Test at exactly-fits and one-over.
- **Deadline.** One caller-supplied absolute `Instant`. Check it before each frame, between chunks of long
  writes, and after each slow scan; no success after expiry.
- **Gates.** Before the first write and at the end of each step: the mount census over the destination
  (`refuse_mounts_within`), then rechecks of every retained staging, `.git` and newly created output chain,
  then the destination-root identity LAST, as in phase G's final gate. Nothing fallible follows the final
  gate's last check except building the result. A replaced or retyped created entry observed at any gate
  refuses (`IdentityChanged`/`DeviceCrossing`), and the final gate re-stats every created entry
  (no-follow, by name under its retained parent pin) against the identity recorded at creation.
- **Phase-G guarantees stay true.** Phase P never writes beneath `repository/.git/` except `index` and
  `sharedindex.*`; never writes config, hooks, refs, or objects; never runs Git. It does not weaken any
  phase-G guard or test. Phase G's own final census (which requires `repository/` to hold only `.git`)
  runs before phase P and is unchanged; phase P adds its own census of the final tree.

## API (crate-private)

Mount the code as a child module of `custody_restore.rs`, like phase G (for example
`#[path = "custody_restore_payload.rs"] mod payload;` with tests in `custody_restore_payload_tests.rs`).
Helpers in `custody_restore_git.rs` that phase P needs (`make_file`, `stream_into`, `observe_deadline`,
`recheck_regular_entry`, `stream_digest`, ...) may be widened to `pub(super)` or moved to a shared child
module, with no behavior change and all 2B3b tests unchanged.

- `restore_payload_plane_v1(plane: RestoredGitPlaneV1, frame_budget: CustodyFrameBudgetV1,
  deadline: Instant) -> Result<RestoredPayloadPlaneV1, Box<PayloadPlaneRestoreFailureV1>>`.
- `PayloadPlaneStepV1`: `Admission, Prevalidation, Worktree, WorktreeModes, Index, Evidence, FinalGate`.
  Progress advances only when a whole step succeeds.
- `RestoredPayloadPlaneV1` retains the `RestoredGitPlaneV1` value unchanged, plus:
  - `worktree: WorktreeInventoryV1`: every created entry (path bytes, kind, mode, length, SHA-256 for files,
    target bytes for symlinks, recorded `dev`/`ino`), and whether the connector was a directory or a root
    gitfile;
  - `index: IndexInventoryV1`: names, lengths, SHA-256, frame modes;
  - `evidence: EvidenceInventoryV1`: per class, every entry with its original frame mode and the mode written;
  - `declared_external_references: Vec<ExternalSymlinkV1>` (plane, path, target bytes);
  - the evidence-directory pins and the worktree directory pins.
- `PayloadPlaneRestoreFailureV1 { cause, last_completed: Option<PayloadPlaneStepV1>,
  payload_materialization_began: bool, ledger_used: u64, partial }`. `partial` keeps the
  `RestoredGitPlaneV1` and every pin and inventory row created so far. Nothing is removed on failure, and
  a second phase-P invocation over the same destination refuses (create-new collision).
- Absent classes: an Empty Worktree, Index, or evidence class creates nothing for that class (no empty
  `evidence/<class>/` directory) and is reported as Empty in the result. `evidence/` itself is created only
  if at least one evidence entry or the root gitfile exists.

## Increments (consecutive, same lane; review cap two source rounds each)

The slice is split before dispatch so each review can converge (2B3b's lesson). Each increment is a
separate implement dispatch on the previous increment's approved commit, with its own two-round cap.

### C1 — primitives and unsafe inventory

The `fs_custody` primitives above, their tests, and the inventory updates. Tests: create a symlink with
an absolute target, a `..` target and a 4095-byte target; refuse an existing name (file, directory,
dangling symlink); refuse when the parent pin was replaced; prove the symlink is never followed (target
points at an existing file whose bytes and mode are unchanged afterwards); exact modes `0`, `0400`, `0555`,
`0755`, `0777` on files and directories regardless of umask (set umask `0o077` and `0o000` in the test
process or a forked child); a mode without owner-write still receives its content through the chosen
shape; the inventory controls fail if a second unsafe site is added (mutation check recorded).

### C2 — worktree plane

Admission (D5, re-verify descriptors, gates), prevalidation of the worktree frame (D3, D6), worktree
materialization into `repository/` with D4 and D7, post-order directory modes, and the per-step and final
gates for this step. Tests on real phase-V-to-G-to-P inputs: the rich clone fixture
(`PlanFixtureV1::rich_clone`) worktree restores byte-exact with exact modes and verbatim symlinks; mode
variants beyond the `0755`/`0644` that `encode_frame` emits (add a mode-aware frame helper); an absolute and
a `..`-escaping symlink both appear in `declared_external_references` and neither is followed; a root
gitfile lands in evidence and not at `repository/.git`; every D3 refusal before any write; a `0555`
directory with children; ledger exactly-fits and one-over; deadline expiry mid-frame; an entry replaced
between creation and the final gate (use the existing test seams) refuses; a device crossing refuses; a
second invocation refuses.

### C3 — index, evidence, final gate and result

Index materialization (D4; any name other than `index` or `sharedindex.*` refuses before writes), the three
evidence classes (D1, D2), the phase-P final gate and the complete `RestoredPayloadPlaneV1` /
`PayloadPlaneRestoreFailureV1`. Tests: index bytes exact including a split index with `sharedindex.*`;
the rich clone's hook (`0755` in the frame) lands `0400` in evidence, never executable, and the original mode
is in the inventory; `a2a-bridge/evidence.json` and `A2A_TASK.md` land only under `evidence/`; planted
config/hook/filter/fsmonitor markers in the evidence classes never run and never appear under `repository/`
(no Git runs in phase P, but assert absence of their side-effect files); every step's typed failure names
the right `last_completed`; an Empty class creates nothing; the one-ledger accounting equals phase V plus
phase G plus phase P's on-disk charge (extend phase G's `assert_one_ledger` equivalent).

## Validation and delivery (each increment)

- Tests first, on real retained production helpers and phase-V/phase-G inputs, with a negative or edge case
  for every new path. Missing-symbol RED is structural only; also show behavioral RED with each guard removed
  (record which ones in the handoff).
- **Local verification per the repo's standing rule:** `cargo fmt --all -- --check`,
  `cargo clippy -p bridge-core --all-targets --all-features -- -D warnings`, and `cargo test -p bridge-core`.
  The full workspace suite is GitHub CI's job and the merge gate. Run tests in the foreground: no background
  jobs, scheduled wakeups, or ending the turn while tests run. `CARGO_INCREMENTAL=0`, proxies unset for
  loopback.
- **Native macOS, non-root, early** (2B3b's lesson: Linux-as-root in the container hid 13 test-only
  assumptions). The controller runs `cargo test -p bridge-core` natively on macOS on each increment's
  candidate BEFORE its source review. Tests must not assume root, case-sensitive file systems, or tight
  wall-clock deadlines started before fixture warm-up; gate Linux-only rows with `#[cfg(target_os = "linux")]`.
- Report exact test totals, ignored and excluded tests, and any failure outside scope (report it, do not
  fix or rebaseline it). Real same-environment base control before attributing any failure.
- Keep `.git/a2a-evidence/2b3c-<increment>` receipts and an eight-section implementation handoff using
  `/Users/wesleyjinks/.claude/handoff-template.md`.
- Stage only owned paths and write `.git/A2A_COMMIT_MSG`; the bridge commits. No agent push or merge.
- Stop and report on a real primitive or contract mismatch instead of inventing semantics or widening scope.

## Out of scope

The `custody-restore.v1` record, the joint hidden-state fixture and the negative-fixture set (2B3d); the
cross-phase restore wrapper and composed failure type (2B3d); linked worktrees, nested repositories,
submodules, LFS, alternates, external evidence, reproducible outputs; any Git execution in phase P; any
checkout, hook, filter, include or remote activation; overwrite, resume, cleanup or deletion; operator or CLI
wiring.

## Files

- crates/bridge-core/src/fs_custody.rs
- crates/bridge-core/src/custody_restore.rs
- crates/bridge-core/src/custody_restore_git.rs (visibility/extraction only)
- crates/bridge-core/src/custody_restore_payload.rs (new)
- crates/bridge-core/src/custody_restore_payload_tests.rs (new)
- crates/bridge-core/src/custody_git_tests.rs (unsafe inventory only)
- crates/bridge-core/src/custody_walk_tests.rs (unsafe inventory only)
- docs/superpowers/reviews/2026-10-14-adr0041-slice2b3c-implementation-handoff.md (new)
