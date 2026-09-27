# ADR-0041 Slice 2B2b2b1 implementation handoff — the coverage plan

**Status:** implemented in the container lane over four bridge turns. **This revision is repair round 1's** (§0.3),
after the Sol implementation review's round 1.
- **First turn.** It implemented the slice.
- **Verify and review then rejected it:**
  - the workspace suite failed one `a2a-bridge` CLI control;
  - the review found that a valid `..`-relative root `.git` gitfile was treated as unresolved.
- **First repair turn.**
  - It fixed the gitfile finding (§0.2).
  - It fixed the flaky CLI control's race in `bin/a2a-bridge/src/main.rs`, a file outside this slice's §8 paths.
- **Verify and review rejected that too:**
  - the suite failed another CLI control, `run_blocking_offloads_off_the_runtime_worker`;
  - the review rejected the out-of-§8 CLI edit, and the red gate as unconfirmed-inherited.
- **Second repair turn (§0.1):**
  - It **reverted the CLI edit byte for byte**, so `bin/` is identical to the base commit and the diff is exactly the
    §8 paths.
  - It **confirmed both failing CLI controls as inherited**, on the base commit's own binary. The timing control's
    exact failure reproduces deterministically there.
- **The Sol implementation review, round 1, rejected the committed slice** (`92bc4a9f`):
  - one WRONG MATERIAL blocker: `cargo-target-v1` excluded a root `target/` that is, or holds, the pinned git
    directory;
  - one MATERIAL SMELL, deferred and folded here: no control discriminated the object sentinel's entry budget.
- **Repair round 1 (§0.3)** fixes the blocker with one precondition conjunct. It adds three controls and three matrix
  rows.
- **Now:**
  - every §5 criterion has a control, and the **74** planner controls and the **26** runner controls pass;
  - the matrix defines **106** rows, and all of them still apply to the final bytes (`matrix.py check`: 0 problems);
  - repair round 1 ran its **3 new rows and 17 related rows** in one foreground run on the final bytes (snapshot
    `5a447c9bd94ad07f`): **20 FLIPPED, 0 NOT-FLIPPED, 0 INADMISSIBLE**. The source was then proved equal to that
    snapshot;
  - the other 86 rows last ran on the second repair turn's bytes (snapshot `690f6c618bf6c58d`, 103 FLIPPED). Those
    bytes differ from the final ones only by this round's two edits.
- Every planner control is red in at least one matrix row (§2, §0.3).
- **The gates repair round 1 ran are green on the final bytes** (§0.3): fmt, clippy, the `bridge-core` lib suite, and
  the workspace suite. The two CLI flakes are inherited and outside this slice (§0.1).
- **Left to CI and the controller:** `cargo deny`, the native ext4 lane, and the macOS lane (§7).

This handoff records evidence only; it claims no review approval.

**Task (authoritative):** `docs/superpowers/plans/2026-09-27-adr0041-slice2b2b2b1-coverage-plan-task.md`, revision 5,
SHA-256 `5853e889ff57cfbb68a6e13282b9fcf4d8937c66db4fc43aeb510f1bec9c94ad` at the base commit. The bridge copy
`.git/A2A_TASK.md` (SHA-256 `6bbd79c6…`) differs from it only in the pinned base, the gate-renamed headings, and the §13
controller notes.

**Clone:** `/Users/wesleyjinks/code/.a2a-implement/impl-33772-mueis64v`
**Branch:** `implement/impl-33772-mueis64v`
**Base HEAD:** `3e12ff5360c40a2abb0be208fd44d33bfbf57fb4` (`main`, the merge of the spec in PR #118)

**Container lane:**
- Linux `7.0.14-orbstack` on **aarch64**, with `/` and `/tmp` on overlayfs, running as uid 0.
- rustc and cargo 1.94.0, and Git 2.54.0 at `/opt/git/bin/git`.
- Every cargo command in this document runs with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target`.
  Every workspace run also unsets `HTTP_PROXY`, `HTTPS_PROXY`, `http_proxy`, and `https_proxy`.
- No dependency was added and no `unsafe` was added. No symlink is followed and no source content is read by path.
  The only Git commands are the new `ls-files --stage -z` and the existing `InitBare` (and admission's `version`).

---

## 0. The repair turns

### 0.1 The verify failures: two inherited CLI flakes, and the reverted out-of-scope edit

**The failures.** Two verify runs each failed one control in `bin/a2a-bridge/src/main.rs`, a different one each time:
- `cli_tests::operator_container_authority_reports_failed_exact_id_removal`: `the exact-ID removal port was not
  invoked`;
- `cli_tests::run_blocking_offloads_off_the_runtime_worker`: `ticker must advance while run_blocking's closure sleeps
  on the blocking pool; got 1`.

**Why this slice cannot reach them.**
- `bin/` is byte-identical to the base commit: `git diff --cached 3e12ff53 -- bin/` is empty.
- Every changed `bridge-core` module (`custody_coverage`, `custody_export`, `custody_git`) is a private `mod` in
  `lib.rs`, so no item this slice changed is reachable from the CLI binary.
- The removal control's code path is `std::process::Command`. `run_blocking` is exactly
  `tokio::task::spawn_blocking(f).await`, defined in `main.rs`.

**Evidence of inheritance.** The base binary is the base commit exported with `git archive` (this clone's branch and
checkout untouched), built in its own target directory `/tmp/target-base`. The current binary is this tree's. Raw
output is in `.git/a2a-bridge/flake/` and `.git/a2a-bridge/flake/inherit/`.

| Control | Base `3e12ff53` binary | This tree's binary |
|---|---|---|
| removal control, whole CLI suite, unloaded | **fails 1 / 25** | fails 2 / 25 (first turn's tree) |
| `run_blocking…`, the process paused (`SIGSTOP`) 8 ms into the test for 200 ms | **fails 19 / 20**: `got 0` 2, `got 1` 10, `got 2` 7 | fails 20 / 20: `got 0` 1, `got 1` 13, `got 2` 6 |
| `run_blocking…`, not paused | passes 10 / 10 | passes 10 / 10 |
| whole CLI suite under 30 busy loops on 15 CPUs | no `run_blocking` failure in 8 runs | no `run_blocking` failure in 8 runs |

The base's whole-suite runs also fail `config::tests::worktrees_config_parses_and_preflight` every time. That is only
because an archive export is not a git repository.

**Mechanisms.**
- **The removal control** writes a shell script, `chmod`s it, and executes it at once. A fork in another test thread
  while the script's write descriptor is open leaves a writable duplicate in that child until it execs. Exec then fails
  `ETXTBSY`, and the port swallows the spawn failure. This is the same race as 2B2a's fixtures (§7, item 4).
- **`run_blocking…`** is a wall-clock assertion: a 5 ms ticker must advance at least 3 times during a 100 ms
  blocking-pool sleep.
  - Any stall of the whole process inside that window fails it, on either binary. The pause reproduction shows this
    with the exact verify message.
  - CPU contention alone does not reproduce it, as the busy-loop row shows. A whole-process stall does, such as
    cgroup CPU throttling, whose default period is 100 ms.
- **`compatibility_descriptor_handoff_validates_objects_before_close`**, a third CLI concurrency flake, failed once in
  25 runs of the first turn's tree. It is in the same unchanged `bin/` code.

**The reverted edit.** The first repair turn added a warm-up exec to the removal control. The review rejected it as
outside the task's owned files, so the second repair turn restored `bin/a2a-bridge/src/main.rs` to the base bytes
(`git show 3e12ff53:bin/a2a-bridge/src/main.rs`) and staged it. This slice therefore leaves both inherited flakes as
the base has them. The recommended fixes, for their owner, are in §7, item 4.

**The gates on the final bytes pass** (§5): the workspace suite ran with 0 failures, and so did `--all-targets`.

### 0.2 The review finding: a `..`-relative gitfile is valid

**The finding.** The first turn compared the gitfile's target to the pinned git directory's canonical path and refused
any `..`. Git's `--relative-paths` worktrees, and any hand-written relative gitfile, name their git directory as
`gitdir: ../…`, so a valid gitfile parked `linked_worktrees`.

**The fix: `resolve_gitfile_target`.** It resolves the target lexically and exactly, as Git does, relative to the
directory holding the gitfile.
- An absolute target is taken as it is.
- A relative target is joined to the worktree's canonical path, and each *leading* `..` removes one component of that
  path. A canonical path holds no symlink, so that removal is the physical parent.
- A `..` after a component the target names itself could cross a symlink, so it is refused. So is a `..` in an absolute
  target, and a `..` that climbs past `/`.

The result must equal the pinned directory's canonical path. That is the path its identity was recorded under, and it is
proved to resolve to the pinned `(dev, ino)` by the pre-write barrier before any write and by every later fresh pin of
the git directory (§8, item 3). No path is stated and no symlink is followed.

**Why the inner-`..` refusal matters.** Real Git 2.54 resolves `gitdir: ../decoy/../pinned.git`, where `decoy` is a
symlink to `other/sub`, to `other/pinned.git`: another repository. Lexically it reads as `../pinned.git`. The control
proves both with `git rev-parse --absolute-git-dir`.

**Controls.**
- `separate_04_a_relative_gitfile_resolves_by_its_leading_parent_components`:
  - a real separate git directory whose gitfile is rewritten to `gitdir: ../pinned.git`, which Git resolves to the
    pinned directory, plans ordinary rows;
  - `gitdir: ../elsewhere.git` parks;
  - the symlink decoy parks;
  - Git's own `git worktree add --relative-paths` gitfile, `gitdir: ../repo/.git/worktrees/linked`, passes the check.
- `separate_05_gitfile_targets_resolve_lexically_and_exactly`: the resolver table.

**RED.** `M-gitfile-leading-dotdot` restores the first turn's behavior, refusing a leading `..` as every `..` was
refused. It turns `separate_04` and `separate_05` red. `M-gitfile-inner-dotdot` admits an inner `..`, which accepts the
decoy, and `M-gitfile-past-root` drops the climb-past-`/` refusal. All three FLIPPED.

### 0.3 Repair round 1: the Sol implementation review, round 1

The review rejected the committed slice (`92bc4a9f`) with one WRONG MATERIAL blocker and one MATERIAL SMELL marked
DEFER. The controller verified both. This round fixes both; the SMELL is folded because it is a cheap, test-only
change. Raw output for the round is in `.git/a2a-bridge/repair-1/`.

#### Finding 1 (WRONG, MATERIAL, blocker): `cargo-target-v1` against a git directory at or beneath the root `target`

**The finding.** The policy precondition checked for regular `Cargo.toml` and `Cargo.lock` files and a directory named
`target`. It did not check where the pinned git directory is, and Git's supported `--separate-git-dir` can place it
there:
- **`--separate-git-dir=repo/target/.gd`:** the worktree walk skipped the whole root `target` before reaching `.gd`.
  The plan labeled `target` reproducible though it holds the git directory, and it had no connector entry.
- **`--separate-git-dir=repo/target`:** connector precedence emitted `target` as `IncludeEntryOnly`, but the skip
  equation still expected one excluded entry. The valid clone was refused with `AccountingMismatch`.

**RED.** Both controls were added first and run against HEAD's unchanged `custody_coverage.rs` (`red.txt`):
- `cargo_05_a_separate_git_dir_that_is_the_root_target_is_its_connector` panicked with
  `plan the source: AccountingMismatch { class: Worktree, expected: 1, observed: 0 }`.
- `cargo_06_a_separate_git_dir_beneath_the_root_target_is_its_connector` planned, but its rows differ in exactly one
  class. The plan has `ReproducibleOutputs { state: ExcludedReproducible, exclusion_id: Some("cargo-target-v1") }`,
  and the control expects `Empty` with no exclusion id.

**The fix: `git_dir_within_target`.** `observe_worktree` gains one precondition conjunct: the policy applies only if
the pinned git directory is neither the root `target` nor beneath it.
- **The test.** The git directory's canonical path must not start with the repository's canonical path joined with
  `target`, compared component by component, so equality counts.
- **Why a canonical prefix is containment.** Both paths are canonical, so neither holds a symlink, and a component
  prefix is physical containment. The git directory's canonical path is the one its identity was recorded under. The
  pre-write barrier, and every later fresh pin, prove it still resolves to the pinned `(dev, ino)` (§8, item 3).
- **Reads and symlinks.** Nothing is stated or read by path, and no symlink is followed.
- **The effect.** With the policy off, `target` is an ordinary directory, captured whole:
  - the worktree walk emits the identity-keyed connector wherever the git directory is (`target` itself, or
    `target/.gd`);
  - the LFS detector descends `target` and prunes the connector by identity;
  - the skip equation expects no skip;
  - `reproducible_outputs` is `empty`, with no exclusion or dependency record.
- **The only production change.** Every consumer reads the one `cargo_target_excluded` flag, so this conjunct is the
  whole fix.

**Controls.** Both use real fixtures from Git 2.54's `init --separate-git-dir`, with a root `Cargo.toml`, `Cargo.lock`,
and `README` committed. Each requires:
- a successful plan with the ordinary rows, `reproducible_outputs` `empty`, and no exclusion or dependency;
- in the worktree frame, re-walked against its receipt, exactly one entry at the git directory's path, a directory,
  with nothing beneath it;
- the §5.4 multiset: every git-directory entry is owned exactly once, and only by a git-directory class.

`cargo_05` also asserts that `target` has the git directory's identity. `cargo_06` also writes build output to
`target/debug/` and requires it captured.

Two fixture facts:
- Git does not create a missing parent for `--separate-git-dir`, so the `target/.gd` fixture creates `target/` first,
  as a Cargo build would.
- Git lists its own separate directory as untracked content, so the fixture adds its files by name.

**Interpretations to check.**
- Containment is decided by canonical path, not by identity. A bind mount could make the root `target` the git
  directory under another canonical path. The policy would then stay on, and the identity-keyed connector would fail
  the skip equation: the plan is refused (`AccountingMismatch`), never falsely excluded.
- A git directory reachable beneath `target` only through a symlink is not physically inside it. Excluding `target` is
  then correct, and the worktree walk never follows the symlink.

#### Finding 2 (SMELL, MATERIAL, DEFER, folded): the sentinel's entry budget had no discriminating control

**The finding.** The object lock sentinel passes the caller's entry budget, and its `EntryLimit` maps to
`object_database` `unresolved` (`ContentUnresolved`). The only low-budget control, `error_04`, is refused earlier by
the census. So an unlimited sentinel budget left every control green.

**The control: `detector_07_the_object_sentinel_walks_under_the_entry_budget`.**
- **The fixture.** A README clone gains 64 loose objects, hashed by Git from files outside the worktree, so `objects/`
  is wider than the census and every other walk.
- **The count.** The sentinel includes every directory beneath `objects/`. So its emitting pass lists exactly the
  entries an independent recursive `std::fs` listing counts; call that N.
- **The bound.** At an entry budget of N, the plan has the ordinary rows. At N − 1, it has the ordinary rows except
  `object_database`, which is `unresolved` (`ContentUnresolved`). The second plan also proves the census and every
  other walk still fit.

**RED.** The control passes on the unchanged code, as the review expected: the mapping was right by inspection, and
there is no production fix. Its RED is the brief's own mutation, which replaces the sentinel's budget with `u64::MAX`.
It ran before the blocker fix, on snapshot `396fc69d02f3173d` (`red-sentinel-budget.txt`). The N − 1 plan's
`object_database` is then `Empty` where `Unresolved [ContentUnresolved]` is expected. (`cargo_05` and `cargo_06` also
failed in that run, because the blocker was not yet fixed.)

#### GREEN

On the final bytes, the three controls pass, and so do the planner suite (**74/74**) and the runner suite (**26/26**)
(`green.txt`).

#### Mutations

Three rows were added to `.git/a2a-bridge/mutation/matrix.py`. "Pure" means `probe_06_every…` and `runner_00`.

| ID | § | Guard mutated | Named red | Named green | Verdict | Every failing control |
|---|---|---|---|---|---|---|
| M-target-git-dir | 3.2 | the conjunct becomes `&& true` | cargo_05, cargo_06 | pure | FLIPPED | cargo_05, cargo_06 |
| M-target-git-dir-beneath | 3.2 | `starts_with(target)` narrows to `eq(&target)` | cargo_06 | cargo_05, pure | FLIPPED | cargo_06 |
| M-sentinel-budget | 3.3 | the sentinel's `self.request.entry_budget` becomes `u64::MAX` | detector_07 | pure | FLIPPED | detector_07 |

`M-target-git-dir-beneath` proves that the predicate's "beneath" half is discriminated on its own: an equality-only
guard leaves `cargo_06` red and `cargo_05` green.

**The final run.** One foreground `matrix.py run` of 20 rows, inside `timeout 590`, ran on snapshot
`5a447c9bd94ad07f` (the final bytes) from 09:59:13Z to 10:04:07Z. All 20 FLIPPED. The rows:
- **the 3 new rows** (above);
- **the 4 rows whose targets lie in the changed `observe_worktree`:** `M-gitfile-check`, `M-target-policy`,
  `M-cargo-rust-toolchain`, and `M-bare-distinct`;
- **13 rows guarding the same policy flag's consumers, the connector, the skip equation, and the sentinel and
  budget:** `M-target-kind`, `M-target-root-only`, `M-lfs-prune-target`, `M-lfs-prune-connector`,
  `M-connector-descends`, `M-connector-by-path`, `M-cargo-content-class`, `M-cargo-digest`, `M-dependency-recheck`,
  `M-equation-off`, `M-sentinel`, `M-sentinel-walk`, and `M-census-budget`.

| ID (re-run) | Verdict | Every failing control |
|---|---|---|
| M-gitfile-check | FLIPPED | separate_02, separate_04 |
| M-target-policy | FLIPPED | cargo_04 |
| M-cargo-rust-toolchain | FLIPPED | cargo_01 |
| M-bare-distinct | FLIPPED | bare_01 |
| M-target-kind | FLIPPED | cargo_02 |
| M-target-root-only | FLIPPED | cargo_03 |
| M-lfs-prune-target | FLIPPED | detector_01 |
| M-lfs-prune-connector | FLIPPED | detector_05 |
| M-connector-descends | FLIPPED | cargo_01, cargo_02, cargo_03, **cargo_05**, **cargo_06**, class_local_01, completeness_01, **detector_07**, separate_01, skip_01 |
| M-connector-by-path | FLIPPED | **cargo_05**, **cargo_06**, separate_01, separate_03 |
| M-cargo-content-class | FLIPPED | cargo_01 |
| M-cargo-digest | FLIPPED | cargo_01 |
| M-dependency-recheck | FLIPPED | class_local_06 |
| M-equation-off | FLIPPED | skip_01, skip_02, and the stray below |
| M-sentinel | FLIPPED | detector_03 |
| M-sentinel-walk | FLIPPED | bare_01, detector_03, **detector_07**, walks_01 |
| M-census-budget | FLIPPED | error_04 |

The bold controls are this round's; they also discriminate the connector rows and the sentinel-walk row.

**Stray.** `M-equation-off` also failed the 2B2a runner control `a8_a8b_a9_a10_bound_streams…`, with
`admit fixture: Spawn(Os { code: 26, kind: ExecutableFileBusy, message: "Text file busy" })`. That is the known
`ETXTBSY` fixture race (§7, item 4), outside the row's guard.

A trial run of `M-target-git-dir` and `M-target-git-dir-beneath` on the same snapshot, at 09:53Z, gave the same
verdicts before the gates ran.

**Source equals the snapshot:** yes.
- **Harness:** the run ended with `VERIFY OK: 6 files equal their snapshot; no pending marker` (10:04:07Z).
  - `matrix.log` has 531 `APPLIED` lines: the prior 508, one pre-fix RED run, two trial rows, and the 20 final rows.
  - It has 535 `RESTORED` lines: 512 + 23.
- **Independent check:** outside the harness, each live file's bytes equal its snapshot copy, and each SHA-256 prefix
  equals the manifest:
  - `custody_coverage.rs` `cff02247`, and `custody_coverage_tests.rs` `30106685`;
  - `custody_git.rs` `71557e65`, and `custody_git_tests.rs` `0aae9351`;
  - `custody_export.rs` `65d99c8e`, and `lib.rs` `3a15b090`.

  `custody_coverage.rs` carries the restore's fresh mtime (10:04:07Z). No `pending.json` exists, and a `/proc` scan
  found no cargo, rustc, test, or harness process left running.
- **After the matrix:** no source file was edited. Only this handoff was written, together with the harness's rendered
  `table.md`.

#### Gates on the final bytes

All gates below ran on snapshot `5a447c9bd94ad07f` before the final matrix run. The matrix restored those bytes
exactly.

| Gate | Exit | Totals |
|---|---|---|
| `cargo fmt --all -- --check` | 0 | no diff |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 0 | no warning or error |
| `cargo test --locked --offline -p bridge-core --lib --no-fail-fast` | 0 | **989 passed**, 0 failed, 0 ignored (986 at `92bc4a9f`, plus 3) |
| `cargo test --locked --offline --workspace --no-fail-fast`, proxies unset | 0 | **4,734 passed**, 0 failed, 13 ignored, over the same 106 `test result` lines as §5 (4,731 plus 3) |

- The `bridge-core` lib output also holds one `test result` line from a pre-existing `fs_custody` control, which
  re-executes the test binary filtered to one test. The total above is the binary's own line.
- The §5 workspace total counted that child line too, so both totals use the same method.
- `cargo deny` stays excluded (§7, item 2).
- The `--all-targets` workspace run and the hygiene check were not re-run this round.

## 1. Structural RED on the predecessor

Before any production code, a scratch test naming `crate::custody_coverage::plan_coverage_v1` and
`crate::custody_git::GitCommandV1::LsFilesStageZ` was appended to `lib.rs` at the base commit.
`cargo test --locked --offline -p bridge-core --lib custody_coverage` failed to compile:

```text
error[E0433]: failed to resolve: could not find `custody_coverage` in the crate root
  --> crates/bridge-core/src/lib.rs:81:24
error[E0599]: no variant or associated item named `LsFilesStageZ` found for enum `GitCommandV1` in the current scope
  --> crates/bridge-core/src/lib.rs:82:51
error: could not compile `bridge-core` (lib test) due to 2 previous errors
```

The scratch lines were removed, restoring `lib.rs` to the base bytes. Raw output:
`.git/a2a-bridge/mutation/structural-red.txt`.

## 2. Behavioral RED

**Order of work, stated plainly.** The runner amendment, the exporter visibility, and `custody_coverage.rs` were written
first, then the controls. So no control was observed red against a predecessor before its guard existed. The behavioral
RED per control comes from removing guards, which §6 allows:

- **`red-all`** (`.git/a2a-bridge/mutation/red-all.json`, raw `output-red-all.txt`) applies the 93 composable rows at
  once. **69 of the 97 controls fail.** The 28 that pass are:
  - the 23 pre-existing 2B2a runner controls, which no row mutates;
  - five planner controls, each masked by a composed row, and each red in its own row:

    | Control | Red in its own row(s) |
    |---|---|
    | `probe_07` | `M-copy-expected`, `M-equation-detectors`, `M-git-lsfiles-argv` |
    | `detector_05` | `M-lfs-prune-connector` (and six routing rows) |
    | `error_01` | `M-scratch-preflight` |
    | `runner_02` | `M-cell-admission` |
    | `runner_05` | `M-cell-listing`, `M-equation-detectors` |
- **The per-row matrix (§6)** turns every one of the 71 planner controls red in at least one row. The three runner
  controls this slice extends (`a6_a7…`, `w2_mutating…`, and the new `ls_files_stage_z…`) turn red under
  `M-git-lsfiles-route` and `M-git-lsfiles-argv`.

**Corrections before the final matrix, stated plainly.** No assertion was relaxed to make a guard pass.
- Some first-run expectations were wrong:
  - Five controls planned fixtures that had committed locally, so their `COMMIT_EDITMSG` is legitimately captured in
    `in_progress_git_operations`. The new `source_rows` helper states that.
  - In `probe_06_every…`, two parser rows expected `Separator` where the strict parser correctly reports `Mode` or
    `ObjectId`, and the 4096-byte path fixture was miscounted.
- One real bug was fixed: a failed `init --bare` reached 2B2's re-measure before its exit check, so it refused as `Io`
  instead of `ProbeInfrastructure`. The exit status is now checked first (§8, item 15).
- `error_03` first injected its I/O refusal *past* the production mapping. It now injects an `FsCustodyError` into the
  listing result, so the production arm `Err(error) => Io` is what it exercises, and `M-census-io` flips it.
- Three controls were added once the first matrix round showed guards without a discriminating control: `separate_03`,
  `ledger_03`, and `alternates_01`. Later additions were `detector_06` (the 1 MiB sink bound) and `bare_01`.

**GREEN:** the planner suite passes **71/71** and the runner suite **26/26** on the final bytes.

## 3. What was built

### 3.1 `custody_git.rs`: the §2.1 command

`GitCommandV1::LsFilesStageZ`, whose argv is exactly `ls-files --stage -z`.
- It includes `GIT_DIR`, as every command but `InitBare` does.
- `permits_object_store_route` is **false** for it.
- Its label is `ls-files --stage -z`.

Nothing else changed: the closed environment, the rooted spawn, the bounds, and the deadline are unchanged, and no
`unsafe` was added, so 2B2a's A14 inventory is unchanged and passes.

**Its controls** (`custody_git_tests.rs`):
- `a6_a7_and_a11d_use_exact_environment_argv_and_init_shape` gains the exact-argv row.
- `w2_mutating_commands_refuse_a_caller_object_store_route` gains `(LsFilesStageZ, false)`.
- A new control, `ls_files_stage_z_lists_a_copied_index_in_a_bare_probe_and_writes_nothing`, reproduces probe P1 for
  SHA-1 and SHA-256. A real repository's index holds a regular file and a `160000` gitlink. It is copied into an
  `InitBare` probe run through the runner. The listing is exactly both NUL-terminated records, with exit 0,
  `GIT_DIR=index-probe.git`, and no object-store route, and the probe tree's digest is unchanged afterwards.

### 3.2 `custody_export.rs`: visibility and behavior-preserving extraction only

| Item | Change |
|---|---|
| `ScratchLedgerV1` and `new`, `reserve`, `reserve_entries`, `release`, `used` | `pub(crate)` |
| `INIT_BARE_FILES_V1`, `INIT_BARE_ENTRIES_V1`, `INIT_BARE_LOGICAL_BYTES_V1` | `pub(crate)` |
| `GitDirectoryBudgetV1`, its four fields, and `for_init` (the init-reservation helper) | `pub(crate)` |
| `PinnedObjectStoreV1::open`, `recheck`, `path`; `pin_alternate_chain` | `pub(crate)` |
| the protected-set helper | extracted as `pub(crate) fn protected_directories(repository, git_dir, primary, alternates)`; the capability's method now calls it |
| `refuse_source_overlap`, `preflight_scratch_root` | `pub(crate)`, and each takes the protected set (`&[&PinnedDirectoryV1]`) instead of the capability; `export_capsule_v1` passes `&capability.protected_directories()` |
| `remeasure_git_directory` | its body extracted as `pub(crate) fn remeasure_git_directory_in(work, budget, ledger)`; the original delegates with `context.work` |

The call sites compute the same values in the same order, so no behavior changed. Every 2B2 control passes (§5).

### 3.3 `custody_coverage.rs` (new, `#[cfg(unix)]`, crate-private)

It is declared in `lib.rs` as `#[cfg(unix)] #[allow(dead_code)] mod custody_coverage;` with a one-line comment, like its
predecessors. It holds no `unsafe`.

**API (§4.1).**

```text
plan_coverage_v1(&CustodyCoverageRequestV1) -> Result<CustodyCoveragePlanV1, CustodyCoverageErrorV1>
```

- **`CustodyCoverageSourcesV1::pin(repository, git_dir, primary_store)`** pins the three roots and the recursive alternate
  chain with 2B2's `PinnedObjectStoreV1::open` and `pin_alternate_chain`. Together these are the full protected set.
- **The request** also holds:
  - the generation id;
  - the frame budget and the entry budget;
  - `CustodyPlanningBudgetV1 { max_scratch_bytes }`;
  - the planning scratch root;
  - the required `CustodyExternalEvidenceV1`, from `NoExternalEvidenceRecordedV1::declare()` or
    `ExternalEvidenceUnresolvedV1::declare()`, with no `Default`;
  - the source object format;
  - the manifest object inventory;
  - the runner route and its deadline.
- **The plan** holds:
  - the canonical 14 rows, in class order;
  - the exclusions and the dependencies;
  - one `CustodyClassReceiptV1 { class, frame_length, frame_sha256, inventory_digest }` per captured, walked class;
  - the `CustodyGitlinkEvidenceV1` (`NoIndex`, `Listed { records, gitlinks }`, or `Unresolved`);
  - the ledger's final charge.
- **`CustodyCoverageErrorV1`** has exactly `InvalidScratch`, `SourceRootDrift`, `PlanningScratchBudget`,
  `ProbeInfrastructure`, `AccountingMismatch`, and `Io`.

**Flow.**
1. **Overlap preflight (§2.2).** 2B2's `preflight_scratch_root` runs over `protected_directories(…)`. It checks
   canonicalize, the path and retained-identity overlap in both directions, owner-private, and empty. Any refusal is
   `InvalidScratch`, and nothing is created.
2. **Pre-write barrier (§2.2).** It runs immediately after, before the ledger exists and before any scratch entry. It
   has four members, each its own check:
   - `pinned_root_unchanged` of the repository;
   - `pinned_root_unchanged` of the git directory;
   - `primary_store.recheck()`, which checks identity and the alternates digest;
   - `recheck()` of every pinned alternate.

   Drift is `SourceRootDrift`.
3. **Census (§3.1).** One listing of the git directory's top level, through its **retained** pin. Every name is routed by
   the closed table `route_git_dir_entry`. The census marks each special row's class, including the non-walked classes
   (`commondir`, `objects.lock`, and so on), and the non-empty `worktrees/`, `modules/`, and `lfs/`.
4. **Worktree constants (§3.2, §3.4).** These apply only when the repository root is not the git directory by identity:
   - The root gitfile check.
   - The `cargo-target-v1` precondition: regular `Cargo.toml` and `Cargo.lock`, and a directory `target`.
   - Under the policy, each dependency file is hashed through the worktree pin, bound to the stat it was read under.
5. **The probe (§2.2–§2.4).** If the census has no `index`, the result is `NoIndex`, with no scratch write. Otherwise:
   1. Charge and create `work/`, `work/home/`, and `work/xdg/`, and admit the runner at `work/`.
   2. Reserve 2B2's `GitDirectoryBudgetV1::for_init("index-probe.git")`: the nine entries and the 4 KiB logical bound.
   3. Run `InitBare { dir: "index-probe.git", object_format }`, check its exit, then remeasure and reconcile with
      `remeasure_git_directory_in`.
   4. Copy `index` and every `sharedindex.*` through the retained git-directory pin with `open_regular_file`. Each file's
      bytes and one entry allowance are reserved before `create_new_regular_child`, and the file joins the expected set.
   5. Run `LsFilesStageZ` with stdout `min(128 × copied + 64 KiB, 256 MiB)`, remeasure, check the exit status, then
      parse.
6. **Class walks (§3.1, §4.2).** Each walk gets its own fresh pin: the six git-directory classes in class order, then
   the worktree. Each goes into an encoder over `io::sink()`. A successful capture walk must satisfy the class-skip
   equation, or the plan refuses `AccountingMismatch`.
7. **Dependency re-stat.** After the worktree walk, each dependency file must still have its bound stat, or
   `reproducible_outputs` is `IdentityChanged`.
8. **Detectors (§3.3).**
   - The LFS detector walks into a `Vec` under a 1 MiB frame budget. Its receipt is explicitly exempt from the equation.
     The planner decodes the frame and scans each `.gitattributes`.
   - `info/attributes` is read through the pin.
   - The object lock sentinel walks `objects/` from a fresh pin.
9. **Alternates.** The git directory's `objects/info/alternates` is read through the pin, and the pinned chain is
   checked for emptiness (§8, item 10).
10. **External evidence**, then **assembly**.

**Selections** (all pure in `(path, stat)` plus plan-start constants):
- `GitDirClassSelectionV1 { class }`. At the top level, it skips an entry another class owns, parks its own special
  rows, and includes the rest as whole subtrees. Below the top level, it parks `*.lock` (`WriterUncontrolled`) and
  `info/grafts` (`ContentUnresolved`).
- `WorktreeSelectionV1 { git_dir, cargo_target_excluded }`:
  - the pinned git directory, by `(dev, ino)` at any depth, is `IncludeEntryOnly`;
  - a root `.git` gitfile is `Include`, and any other root `.git` parks;
  - the root `target` is skipped only under the policy and only as a directory;
  - a root `.gitmodules` and a nested `.git` park.
- `LfsDetectorSelectionV1` skips the connector by identity, every `.git`, and the excluded root `target`. It includes
  every other directory and every `.gitattributes`, and skips every other file.
- `ObjectLockSentinelV1` includes directories, parks a `*.lock` file, and skips every other file unread.

**Test seam** (`#[cfg(test)] mod seam`, thread-local). It provides:
- a log of every walk's `Begin`/`End` with its pin's serial;
- a log of every completed probe run's stage, argv, `GIT_DIR`, and exit status;
- a point hook at `BarrierPassed`, `CensusTaken`, and `WorktreeWalked`;
- an injected `CustodyGitError` for one probe stage (the shared runner seam of §4.5);
- an injected census listing failure that flows through the production mapping.

The controls also reuse 2B2b2a's walker seam, for a device boundary and for drift.

## 4. The §5 controls

All are in `custody_coverage::tests` (`crates/bridge-core/src/custody_coverage_tests.rs`), except the three runner
controls in §3.1.
- Every control plans a real repository built with `/opt/git/bin/git`, isolated from host configuration.
- Every probe child runs through 2B2a's runner under the test-system route profile, or a fixture route whose
  `init` is the real Git.
- The fixture route performs one warm-up exec before use, which proves no forked child still holds its write descriptor
  (§7, item 4).

| §5 | Control(s) | What it proves |
|---|---|---|
| 1 | `probe_01` | SHA-1 and SHA-256 repositories each reach exactly one `init --bare --template= --object-format=<format> index-probe.git` without `GIT_DIR`, then exactly one `ls-files --stage -z` with `GIT_DIR=index-probe.git` and exit 0, listing `{records: 1, gitlinks: 0}`. `InitBare { dir: "work/index-probe.git" }` refuses `InvalidCommand` before any spawn |
| 1 | `probe_02` | a `160000` index entry with no `.gitmodules` makes nested `unresolved` (`DependencyUnresolved`), with gitlinks 1 |
| 1 | `probe_03` | a split index lists normally with its `sharedindex.*` copied; with the shared index deleted, `ls-files` fails and `index` is `unresolved` (`ContentUnresolved`) |
| 1 | `probe_04` | a fixture listing that exits 1 with empty stdout makes `index` `unresolved` |
| 1 | `probe_05` | exactly the stdout bound (valid records) lists; one byte more is `StdoutLimit`, and `index` is `unresolved`. `probe_stdout_limit(u64::MAX)` is 256 MiB |
| 1 | `probe_06_every…`, `probe_06_a_malformed…` | the parser refuses every shape: trailing bytes, the mode (short, long, non-octal), each separator, the object id (short, long, uppercase, non-hex, wrong format length), the stage, empty, absolute, `//`, `..`, `.`, and trailing-slash paths, a path over 4096 bytes, and short records. It admits a path of exactly 4096 bytes and counts `160000`. A malformed probe listing leaves `index` `unresolved` |
| 1 | `probe_07` | the source `index` bytes, mtime (seconds and nanoseconds), and inode are unchanged by a plan |
| 2 | `ledger_01` | the ledger equals an independent census of the planning scratch (entries × 64 KiB + file bytes): 13 entries plus `HEAD`, `config`, and the index |
| 2 | `ledger_02` | the exact budget succeeds and one byte less refuses at the copy's reservation, with no byte copied. The init peak (12 entries + 4 KiB) admits the spawn, and one byte less refuses before the probe exists. The layout's three entries have the same max+1 boundary |
| 2 | `ledger_03` | a probe child that writes into the probe repository and exits 0 is refused `PlanningScratchBudget` by the post-exit re-measure |
| 2 | `overlap_01` | an empty owner-private descendant of an alternate store, the store renamed after pinning (an identity alias its path cannot show), and a scratch inside the worktree each refuse `InvalidScratch` with no entry created |
| 2 | `barrier_01`–`barrier_06` | one row per change, each after request construction: replace the repository, the git directory, the primary store, or a pinned alternate (identities), rewrite the primary alternates file, and rewrite a non-primary alternate's alternates file to name the store containing the scratch. Each refuses `SourceRootDrift` with the scratch still empty. Each replacement keeps the other members' identities, so only its own member can catch it |
| 2 | `barrier_07` | the unchanged chain plans, and `alternates_and_shared_stores` is `unresolved` |
| 3 | `table_01` | every §3.1 row routes as specified, including the prefix rows, 12 `*.lock` names, `gc.pid`, `commondir`, `shallow`, and 9 unknown names |
| 3 | `table_02_*` (9) | real repositories with `commondir`, `shallow`, `info/grafts`, `index.lock`, `gc.pid`, `objects.lock`, `refs/heads/main.lock`, an unknown name, and non-empty (then empty) `worktrees/`, `modules/`, and `lfs/`, each giving its state and reason |
| 3 | `table_03_rerere…`, `table_03_linked…` | real rerere (`MERGE_RR`), notes-merge (`NOTES_MERGE_*`), and bisect (`BISECT_*`) states are in the re-walked `in_progress_git_operations` frame. For a real `git worktree add`, the main repository's `worktrees/` and the linked git directory's `commondir` each park linked worktrees |
| 3 | `alternates_01` | the alternates file alone, and the pinned chain alone, each park alternates; comments alone do not |
| 4 | `completeness_01` | a rich clone: a dirty worktree, untracked, ignored, a stash, reflogs, an in-progress merge with rerere, a hook, `info/sparse-checkout`, bridge evidence, and a worktree file named `HEAD`. All seven receipts re-walk to byte-identical frames. The union of frames keyed by `(RootDomain, path)`, plus `objects/**`, equals an independent `std::fs` listing, every key exactly once |
| 5 | `separate_01` | `--separate-git-dir=repo/.gd` makes `.gd` a childless directory entry and `.git` a regular gitfile in the worktree frame. The multiset holds every physical entry once |
| 5 | `separate_02`, `separate_03` | a gitfile naming another directory parks linked worktrees, and naming the pinned directory does not. A root `.git` directory other than the pinned one parks the worktree and linked worktrees |
| 5 | `separate_04`, `separate_05` (repair turn) | a `..`-relative gitfile resolves exactly, Git's `--relative-paths` gitfile included; a relative gitfile naming another directory, and a symlink decoy behind an inner `..`, park linked worktrees (§0.2) |
| 6 | `detector_01` | a root `target/` with 2,000 files and a `filter=lfs` `.gitattributes`, under an entry budget of 600, plans with lfs `empty` |
| 6 | `detector_02_*` | a nested `.gitattributes`, and `info/attributes`, with `filter=lfs` each park lfs; without the filter, nothing parks |
| 6 | `detector_03` | `objects/info/commit-graph.lock` parks `object_database` (`WriterUncontrolled`) |
| 6 | `detector_04`, `detector_05` | the detector prunes a nested `.git` (its `filter=lfs` is never read) and the in-worktree `.gd` by identity |
| 6 | `detector_06` | a 1 MiB + 1 byte `.gitattributes` exceeds the sink, so the worktree is `unresolved`; 512 KiB does not |
| 6 | `detector_07` (repair round 1) | with 64 extra loose objects, an entry budget of exactly the sentinel's independently counted N plans the ordinary rows; N − 1 leaves only `object_database` `unresolved` (`ContentUnresolved`) (§0.3) |
| 7 | `skip_01` | an ordinary clone with a `README` and no attributes plans, with a complete multiset. As a negative control, the LFS detector's own receipt skips 2 and fails `require_class_skips(Worktree, 0, …)` |
| 7 | `skip_02` | an `ORIG_HEAD` created after the census (seam) refuses `AccountingMismatch { class: Index }` |
| 8 | `cargo_01` | the row, the exact `CustodyExclusionV1("cargo-target-v1", "reproducible_outputs", "cargo-target-v1", […])`, and the three `worktree-file` dependencies by SHA-256. `"reproducible_outputs"` is asserted equal to the serde wire name. The records assemble into a `CustodyManifestV1` that validates. The dependency files are in the worktree frame and `target` is not. Changing `Cargo.lock` changes only its digest |
| 8 | `cargo_02`, `cargo_03`, `cargo_04` | a regular-file root `target` and a symlink root `target` are captured, with no records; a nested `target` is captured; with only `Cargo.toml`, the root `target` is ordinary content |
| 8 | `cargo_05`, `cargo_06` (repair round 1) | a real `--separate-git-dir` at `repo/target`, and at `repo/target/.gd`, each plan with no `cargo-target-v1` record, one childless connector in the worktree frame, and the git directory's contents owned only by git-directory classes (§0.3) |
| 9 | `objects_01` | an empty initialized repository is `empty`, and a one-object repository with that object in the inventory is `captured`. No receipt and no class walk ever names `object_database` |
| 10 | `error_01`–`error_04` | `InvalidScratch` (non-empty, mode 0755, missing); `SourceRootDrift` (the worktree replaced after the barrier, caught by the fresh pin); `Io` (the census listing fails); a census over the entry budget leaves the ten census classes `unresolved` with no scratch write. `PlanningScratchBudget` is `ledger_02` and `ledger_03`; `AccountingMismatch` is `skip_02` |
| 10 | `runner_00`–`runner_07` | the runner-failure table: every stage × every one of the 18 variants as a pure mapping (`runner_00`), then one control per distinct cell through the shared seam and real fixtures. The cells: the 14 infrastructure variants at each stage; admission's four child outcomes; `init`'s four and its nonzero exit; an `index` that is a symlink (copy open failure); the listing's four and a real timeout; and no index (no run, no write) |
| 10 | `class_local_01`–`06` | one class-local case per reason. A Unix socket in `hooks/` is `ContentUnresolved`. `.gitmodules` is `DependencyUnresolved`. A device boundary (walker seam) is `MountBoundary`. A file rewritten during the final pass (walker seam) is `IdentityChanged`. External evidence is required. `Cargo.lock` rewritten after its capture is `IdentityChanged`. `WriterUncontrolled` is `table_02_a_top_level_lock` |
| 11 | `walks_01` | nine walks run strictly one after another (each `Begin` is followed by its own `End`), each on a distinct pin serial, in class order, then the worktree, the LFS detector, and the sentinel |
| — | `bare_01` | a bare clone plans its git directory only, with no worktree or LFS walk |

## 5. Verification totals

**Repair round 1's gates, on its final bytes, are in §0.3.** The table below is the second repair turn's run, kept as
the baseline its totals are compared against.

All gates ran on the second repair turn's final bytes (snapshot `690f6c618bf6c58d`, with `bin/` identical to the
base), before its final matrix round, and no source byte changed afterwards. Raw output is in `.git/a2a-bridge/gates/`. Totals count every `test result` line.

| Gate | Exit | Totals |
|---|---|---|
| `cargo test --locked --offline -p bridge-core --lib custody_coverage` | 0 | **71 passed**, 0 failed, 0 ignored |
| `cargo test --locked --offline -p bridge-core --lib custody_git_tests` | 0 | **26 passed**, 0 failed (2B2a's 25 plus the new control) |
| `cargo test --locked --offline -p bridge-core --no-fail-fast` | 0 | **1,124 passed**, 0 failed, 0 ignored: 2B2b2a's 1,052 plus 72 |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast` | 0 | 90 test binaries: **4,721 passed**, 0 failed, 13 ignored: 2B2b2a's 4,649 plus 72 |
| `cargo test --locked --offline --workspace --no-fail-fast` | 0 | 90 test binaries and 16 doctest runs: **4,731 passed**, 0 failed, 13 ignored: 2B2b2a's 4,659 plus 72 |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 0 | no warning or error |
| `cargo fmt --all -- --check` | 0 | no diff |
| `git diff --check` | 0 | no output; `git diff --cached --check` over the staged paths is in §9 |
| `cargo deny check` | — | **excluded**: not installed (`error: no such command: deny`, exit 101); CI runs it (§7) |
| `cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene` | 0 | `repository hygiene validated`; `tracked_artifacts: 41`, `validated_example_configs: 9` |

The 13 ignored tests are the pre-existing live and e2e tests outside `bridge-core`.

## 6. Mutation matrix

**Harness:** `.git/a2a-bridge/mutation/matrix.py` (untracked; it survives the container). It keeps:
- the log in `matrix.log`;
- one JSON record per row in `results.jsonl`, keyed by snapshot;
- raw cargo output in `output-<id>.txt`;
- the snapshot of all six changed source files in `snapshot/`, with `manifest.json`. The first repair turn added
  `bin/a2a-bridge/src/main.rs`; the second removed it again, when the file was restored to the base;
- the full table in `table.md`, and the compact one below in `compact-table.md`.

**Rules** (2B2b2a style):
- A row applies only if each of its targets occurs exactly once in the snapshot. On the final bytes, `matrix.py check`
  reports 106 rows, 96 composable, 0 problems, and 100 controls (round 5: 103 rows, 93 composable).
- A `pending.json` marker is written and `fsync`ed before a mutation is applied. It is removed only after:
  1. the source is rewritten from the snapshot and `fsync`ed;
  2. its mtime is refreshed with `os.utime` (now);
  3. it re-hashes equal to the snapshot.

  Any later invocation that finds the marker restores first. SIGINT, SIGTERM, and SIGHUP restore before exiting.
- Each row runs `cargo test --locked --offline -p bridge-core --lib --no-fail-fast -- custody_coverage::
  custody_git_tests::` with a 240 s cap. That is all 97 controls.
- **Foreground only.** Every call ran in the foreground inside `timeout 590`. No background job was started.

**Verdict rule.** `FLIPPED` requires all of these:
- the crate compiled, with no timeout;
- every named test ran;
- every named red control failed;
- every named green control passed. The greens are pure-function controls, or the other barrier rows for a barrier
  member.

Anything else is `INADMISSIBLE` or `NOT-FLIPPED`.

**Rounds.**
1. **Round 1** (snapshot `21da8a2532f4508f`, 97 rows). 96 FLIPPED. `M-charge-init-reconcile` was **NOT-FLIPPED**, and
   the source did not change. The row was mislabeled:
   - Without the init reconcile, the post-listing re-measure still reconciles the same budget, so `ledger_01`'s final
     census matches.
   - But the exact-budget plan of `ledger_02` is refused, because the unreconciled 4 KiB blocks the copy.

   The row's named red became `ledger_02`, and its rerun FLIPPED.
2. **Round 2** (snapshot `3c7acd762a6072ff`, 100 rows). It added `M-scratch-preflight`, `M-census-io`, and
   `M-bare-distinct` after the census seam fix and `bare_01` (§2). 100 FLIPPED.
3. **Round 3** (snapshot `88fbd7c7cc024112`, the first turn's final bytes). This snapshot is round 2's bytes plus the
   fixture warm-up and the wider real-timeout deadline (§7, item 4). 100 FLIPPED, then `red-all`.
4. **Round 4, first repair turn** (snapshot `10849568a3a17f71`, 103 rows). It adds `M-gitfile-leading-dotdot`,
   `M-gitfile-inner-dotdot`, and `M-gitfile-past-root` (§0.2), and snapshots the since-reverted `main.rs`. 103 FLIPPED,
   then `red-all`.
5. **Round 5, second repair turn, final** (snapshot `690f6c618bf6c58d`, 103 rows). The bytes are round 4's
   `bridge-core` bytes; only `main.rs` left the diff. **103 FLIPPED, 0 NOT-FLIPPED, 0 INADMISSIBLE, 0 not run**, then
   `red-all`: 93 composable rows applied, and 69 of 97 controls fail.
6. **Round 6, repair round 1, final** (snapshot `5a447c9bd94ad07f`, 106 rows). It adds `M-target-git-dir`,
   `M-target-git-dir-beneath`, and `M-sentinel-budget`, and re-runs the 17 related rows: **20 FLIPPED, 0 NOT-FLIPPED,
   0 INADMISSIBLE**. The other 86 rows keep their round 5 verdicts. No `red-all` was run. The rows, the restore proof,
   and the stray are in §0.3.

**Source equals snapshot after the matrix** (round 5; round 6's proof is in §0.3): yes.
- **Harness:** the last call ended with `VERIFY OK: 6 files equal their snapshot; no pending marker` (09:13:24Z).
  `matrix.log` has 508 `APPLIED` lines (round 1: 97 rows, 1 rerun, and `red-all`; rounds 2–5: 100, 101, 104, and 104)
  and 512 `RESTORED` lines, because each `red-all` restores two files.
- **Independent check:** outside the harness, each file's SHA-256 prefix equals the manifest: `custody_coverage.rs`
  `f13c1e22`, `custody_coverage_tests.rs` `f2c0af80`, `custody_git.rs` `71557e65`, `custody_git_tests.rs` `0aae9351`,
  `custody_export.rs` `65d99c8e`, and `lib.rs` `3a15b090`. No `pending.json` exists.
- **After the matrix:** no snapshot file was edited; only this handoff was written.

**Every §6 minimum row is present:**
- each table row mis-routed: 13 `M-route-*` rows;
- each special row off: `M-special-*` (8), `M-census-marks`, and `M-census-budget`;
- the probe exit check: `M-probe-exit`, and for init, `M-init-exit`;
- the probe ledger charges: the seven `M-charge-*` rows, plus `M-copy-expected` and `M-remeasure-listing`;
- the parser leniencies: `M-parse-*` (8);
- `160000` ignored: `M-160000` and `M-gitlink-mark`;
- the connector descending, or keyed by path: `M-connector-descends` and `M-connector-by-path`, with the gitfile rows
  `M-gitfile-check`, `M-gitfile-leading-dotdot`, `M-gitfile-inner-dotdot`, and `M-gitfile-past-root`;
- the target kind check: `M-target-kind`, and the git-directory containment check, `M-target-git-dir` and
  `M-target-git-dir-beneath` (repair round 1);
- each detector's pruning: `M-lfs-prune-connector`, `-nested`, and `-target`;
- the object sentinel: `M-sentinel`, `M-sentinel-walk`, and its entry budget, `M-sentinel-budget` (repair round 1);
- the class-skip equation off, or applied to detectors: `M-equation-off` and `M-equation-detectors`;
- the error-versus-row mapping swapped: `M-error-row-walk`, `M-cell-parse`, and the six `M-cell-*` rows (one per
  §4.5 cell);
- walks made concurrent: `M-walks-concurrent`, which runs the six git-directory walks on scoped threads, and
  `M-walks-shared-pin`.

§5.2 asks for one mutation per recheck member. The members are the barrier's four checks (§8, item 17). Each member's
row turns red exactly its own rows and leaves every other barrier row green, and the verdict rule enforces that through
the named greens.

**Stray failures.** In round 5, five rows each also failed one pre-existing 2B2a runner control:
- `M-special-dependency-dirs` and `M-overlap-alternates` failed `ir1w3…`;
- `M-alternates-chain` failed `a8_a8b_a9_a10_bounded…`;
- `M-reason-drift` failed `w1…`;
- `M-160000` failed `a5g…`.

The first four are `Spawn(… ExecutableFileBusy, "Text file busy")` in 2B2a's own fixture admission, the known `ETXTBSY`
race (§7, item 4). `a5g…`'s failing assertion expects `UnsupportedVersion` from a freshly written fixture script. A
`Spawn(ETXTBSY)` refusal fails that assertion without printing the error, so that failure is consistent with the same
race. Rounds 1–4 showed 13 such strays in about 400 runs, always in a 2B2a fixture control and never in a planner
control; every one whose output survives was the same `ETXTBSY`.

| ID | § | Guard mutated | Named red | Verdict | Failing |
|---|---|---|---|---|---|
| M-route-refs | 3.1 | HEAD, ORIG_HEAD, FETCH_HEAD, packed-refs, refs/, shallow route to refs_and_head | table_01 | FLIPPED | 33 |
| M-route-logs | 3.1 | logs/ routes to stash_and_reflogs | table_01 | FLIPPED | 31 |
| M-route-index | 3.1 | index routes to index | table_01 | FLIPPED | 30 |
| M-route-sharedindex | 3.1 | sharedindex.* routes to index | table_01 | FLIPPED | 1 |
| M-route-config | 3.1 | config, hooks/, info/, ... route to git_configuration_and_hooks | table_01 | FLIPPED | 32 |
| M-route-in-progress | 3.1 | MERGE_*, ..., sequencer/ route to in_progress_git_operations | table_01, table_03_rerere | FLIPPED | 9 |
| M-route-bisect | 3.1 | BISECT_* routes to in_progress_git_operations | table_01, table_03_rerere | FLIPPED | 2 |
| M-route-bridge | 3.1 | a2a-bridge/ routes to bridge_evidence | table_01 | FLIPPED | 2 |
| M-route-a2a | 3.1 | A2A_* routes to bridge_evidence | table_01 | FLIPPED | 1 |
| M-route-objects | 3.1 | objects/ routes to object_database (never framed) | table_01, completeness_01 | FLIPPED | 9 |
| M-route-worktrees | 3.1 | worktrees/ and commondir route to linked_worktrees | table_01, table_02_non_empty | FLIPPED | 4 |
| M-route-modules | 3.1 | modules/ routes to nested_repositories_and_submodules | table_01, table_02_non_empty | FLIPPED | 2 |
| M-route-lfs | 3.1 | lfs/ routes to lfs_and_external_payloads | table_01, table_02_non_empty | FLIPPED | 2 |
| M-special-commondir | 3.1 | commondir parks linked_worktrees DependencyUnresolved | table_01, table_02_commondir, table_03_linked | FLIPPED | 3 |
| M-special-shallow | 3.1 | shallow makes refs_and_head ContentUnresolved | table_01, table_02_shallow | FLIPPED | 2 |
| M-special-grafts | 3.1 | info/grafts makes git_configuration_and_hooks ContentUnresolved | table_02_info_grafts | FLIPPED | 1 |
| M-special-top-lock | 3.1 | a top-level *.lock parks its stem class WriterUncontrolled | table_01, table_02_a_top_level_lock, table_02_a_lock_of_a_non_walked | FLIPPED | 3 |
| M-special-gc-pid | 3.1 | gc.pid parks git_configuration_and_hooks WriterUncontrolled | table_01, table_02_gc_pid | FLIPPED | 2 |
| M-special-nested-lock | 3.1 | a *.lock below the top level parks the enclosing class | table_02_a_nested_lock | FLIPPED | 1 |
| M-special-unknown | 3.1 | an unknown name makes git_configuration_and_hooks ContentUnresolved | table_01, table_02_an_unknown | FLIPPED | 3 |
| M-special-dependency-dirs | 3.1 | non-empty worktrees/, modules/, lfs/ park their classes | table_02_non_empty, table_03_linked | FLIPPED | 3 |
| M-census-marks | 3.1 | the census marks each special row's class (non-walked classes included) | table_02_commondir, table_02_a_lock_of_a_non_walked | FLIPPED | 3 |
| M-census-budget | 4.3 | a census over the entry budget leaves every census class unresolved | error_04 | FLIPPED | 1 |
| M-connector-descends | 3.2 | the pinned git directory is an IncludeEntryOnly connector | separate_01, completeness_01 | FLIPPED | 7 |
| M-connector-by-path | 3.2 | the connector is keyed by identity, not by path | separate_01 | FLIPPED | 2 |
| M-root-dotgit | 3.2 | a root .git that is neither the connector nor a gitfile parks | separate_03 | FLIPPED | 1 |
| M-gitfile-check | 3.2 | a root gitfile must name the pinned git directory | separate_02 | FLIPPED | 2 |
| M-gitfile-leading-dotdot | 3.2 | a relative gitfile's leading .. removes a canonical-root component | separate_04, separate_05 | FLIPPED | 2 |
| M-gitfile-inner-dotdot | 3.2 | a .. behind a component the target names is refused | separate_04, separate_05 | FLIPPED | 2 |
| M-gitfile-past-root | 3.2 | a leading .. that climbs past / is refused | separate_05 | FLIPPED | 1 |
| M-target-kind | 3.2 | only a directory root target is policy-excluded (SMELL-1) | cargo_02 | FLIPPED | 1 |
| M-target-root-only | 3.2 | only the root target is excluded; a nested target is captured | cargo_03 | FLIPPED | 1 |
| M-target-policy | 3.4 | the policy needs both Cargo.toml and Cargo.lock as regular files | cargo_04 | FLIPPED | 1 |
| M-gitmodules | 3.2 | a root .gitmodules parks nested_repositories_and_submodules | class_local_02 | FLIPPED | 1 |
| M-nested-git | 3.2 | a .git below the root, other than the pinned one, parks | detector_04 | FLIPPED | 1 |
| M-park-evidence | 3.2 | a worktree park marks the class it evidences | class_local_02, detector_04, separate_03 | FLIPPED | 3 |
| M-lfs-scan | 3.3 | a .gitattributes naming filter=lfs parks lfs | detector_02_a_nested | FLIPPED | 1 |
| M-lfs-info | 3.3 | info/attributes naming filter=lfs parks lfs | detector_02_info | FLIPPED | 1 |
| M-lfs-prune-connector | 3.3 | the LFS detector prunes the pinned git directory by identity | detector_05 | FLIPPED | 1 |
| M-lfs-prune-nested | 3.3 | the LFS detector prunes every nested .git | detector_04 | FLIPPED | 1 |
| M-lfs-prune-target | 3.3 | the LFS detector prunes the excluded root target | detector_01 | FLIPPED | 1 |
| M-lfs-sink | 3.3 | the LFS detector sink is bounded at 1 MiB | detector_06 | FLIPPED | 1 |
| M-sentinel | 3.3 | the object sentinel parks on *.lock under objects/ | detector_03 | FLIPPED | 1 |
| M-sentinel-walk | 3.3 | the object sentinel walk runs | detector_03, walks_01 | FLIPPED | 3 |
| M-alternates-chain | 3.1 | a non-empty pinned alternate chain parks alternates | alternates_01 | FLIPPED | 2 |
| M-alternates-file | 3.1 | a non-comment objects/info/alternates line parks alternates | alternates_01 | FLIPPED | 1 |
| M-no-index | 2.2 | no source index runs no probe (NoIndex) | runner_07 | FLIPPED | 3 |
| M-init-exit | 4.5 | a nonzero init --bare exit refuses ProbeInfrastructure | runner_03 | FLIPPED | 1 |
| M-probe-exit | 2.3 | the probe exit status is checked before parsing | probe_04, probe_03 | FLIPPED | 2 |
| M-probe-limit | 2.3 | the probe stdout limit is exactly the bound | probe_05 | FLIPPED | 1 |
| M-probe-limit-formula | 2.3 | the bound is 128 x copied bytes + 64 KiB | probe_05 | FLIPPED | 1 |
| M-probe-sharedindex | 2.2 | every sharedindex.* is copied with the index | probe_03 | FLIPPED | 1 |
| M-160000 | 2.4 | a 160000 record is a gitlink | probe_02, probe_06_every | FLIPPED | 3 |
| M-gitlink-mark | 2.4 | a gitlink parks nested_repositories_and_submodules | probe_02 | FLIPPED | 1 |
| M-parse-mode | 2.4 | parser: mode is six octal digits | probe_06_every | FLIPPED | 1 |
| M-parse-space-1 | 2.4 | parser: a space follows the mode | probe_06_every | FLIPPED | 1 |
| M-parse-oid | 2.4 | parser: the object id is lowercase hex | probe_06_every | FLIPPED | 1 |
| M-parse-space-2 | 2.4 | parser: a space follows the object id | probe_06_every | FLIPPED | 1 |
| M-parse-stage | 2.4 | parser: the stage is 0 to 3 | probe_06_every | FLIPPED | 1 |
| M-parse-tab | 2.4 | parser: a tab precedes the path | probe_06_every | FLIPPED | 1 |
| M-parse-path | 2.4 | parser: the path satisfies the 2B2b1 path policy | probe_06_every | FLIPPED | 1 |
| M-parse-trailing | 2.4 | parser: no bytes follow the last NUL | probe_06_every | FLIPPED | 1 |
| M-cell-infra | 4.5 | cell 1: an infrastructure refusal at the listing stage refuses the plan | runner_00, runner_01 | FLIPPED | 2 |
| M-cell-admission | 4.5 | cell 2: admission child outcomes refuse the plan | runner_00, runner_02 | FLIPPED | 2 |
| M-cell-init | 4.5 | cell 3: init --bare child outcomes refuse the plan | runner_00, runner_03 | FLIPPED | 2 |
| M-cell-copy | 4.5 | cell 4: an index copy open failure is class-local | runner_04 | FLIPPED | 1 |
| M-cell-listing | 4.5 | cell 5: listing child outcomes are class-local | runner_00, runner_05 | FLIPPED | 3 |
| M-cell-parse | 4.5 | cell 6: a malformed listing is class-local | probe_06_a_malformed | FLIPPED | 1 |
| M-charge-work | 2.2 | ledger: work/ is charged | ledger_01, ledger_02 | FLIPPED | 2 |
| M-charge-home-xdg | 2.2 | ledger: home/ and xdg/ are charged | ledger_01, ledger_02 | FLIPPED | 2 |
| M-charge-init-entries | 2.2 | ledger: the nine init entries are reserved before the spawn | ledger_01, ledger_02 | FLIPPED | 2 |
| M-charge-init-logical | 2.2 | ledger: the logical HEAD/config bound is reserved before the spawn | ledger_01, ledger_02 | FLIPPED | 2 |
| M-charge-init-reconcile | 2.2 | ledger: the init reservation is remeasured and reconciled | ledger_02 | FLIPPED | 1 |
| M-charge-copy-bytes | 2.2 | ledger: each copied file's bytes are reserved before the copy | ledger_01, ledger_02 | FLIPPED | 2 |
| M-charge-copy-entry | 2.2 | ledger: each copied file's entry allowance is reserved | ledger_01, ledger_02 | FLIPPED | 2 |
| M-copy-expected | 2.2 | a copied file joins the probe's expected file set | ledger_01, probe_01 | FLIPPED | 48 |
| M-remeasure-listing | 2.2 | the probe child is remeasured after it exits | ledger_03 | FLIPPED | 1 |
| M-overlap | 2.2 | the overlap preflight covers the protected set | overlap_01 | FLIPPED | 1 |
| M-overlap-alternates | 2.2 | the protected set includes every pinned alternate store | overlap_01 | FLIPPED | 2 |
| M-barrier-repo | 2.2 | barrier member: the source repository's identity | barrier_01 | FLIPPED | 1 |
| M-barrier-gitdir | 2.2 | barrier member: the git directory's identity | barrier_02 | FLIPPED | 1 |
| M-barrier-primary | 2.2 | barrier member: the primary store's identity and alternates digest | barrier_03, barrier_05 | FLIPPED | 2 |
| M-barrier-alternates | 2.2 | barrier member: every pinned alternate's identity and alternates digest | barrier_04, barrier_06 | FLIPPED | 2 |
| M-fresh-pin-identity | 4.2 | a fresh pin must be the pinned directory | error_02 | FLIPPED | 1 |
| M-walks-shared-pin | 4.2 | each walk has its own fresh pin | walks_01 | FLIPPED | 1 |
| M-walks-concurrent | 4.2 | walks run strictly one at a time | walks_01 | FLIPPED | 1 |
| M-equation-off | 4.3 | the class-skip equation refuses a mismatch | skip_02 | FLIPPED | 2 |
| M-equation-detectors | 4.3 | detector receipts are exempt from the class-skip equation | skip_01 | FLIPPED | 50 |
| M-error-row-walk | 4.5 | a walker refusal is an unresolved row, never a plan error | class_local_01 | FLIPPED | 9 |
| M-reason-mount | 4.3 | a device boundary is MountBoundary | class_local_03 | FLIPPED | 1 |
| M-reason-drift | 4.3 | walker drift is IdentityChanged | class_local_04 | FLIPPED | 2 |
| M-receipts-empty | 4.3 | a walked class with no entries is empty and has no receipt | skip_01 | FLIPPED | 30 |
| M-object-inventory | 4.2 | object_database is captured only for a non-empty inventory | objects_01 | FLIPPED | 1 |
| M-external | 4.4 | ExternalEvidenceUnresolvedV1 makes external_evidence unresolved | class_local_05 | FLIPPED | 1 |
| M-scratch-preflight | 2.2 | the planning scratch is preflighted owner-private and empty | error_01, overlap_01 | FLIPPED | 2 |
| M-census-io | 4.5 | an I/O failure listing the pinned git directory is Io | error_03 | FLIPPED | 1 |
| M-bare-distinct | 3.2 | a bare source has no worktree walk | bare_01 | FLIPPED | 1 |
| M-cargo-content-class | 3.4 | content_class is exactly "reproducible_outputs" | cargo_01 | FLIPPED | 1 |
| M-cargo-rust-toolchain | 3.4 | a regular rust-toolchain.toml is a dependency | cargo_01 | FLIPPED | 1 |
| M-cargo-digest | 3.4 | each dependency binds the SHA-256 of its file | cargo_01 | FLIPPED | 1 |
| M-dependency-recheck | 3.4 | a dependency changed after its capture is IdentityChanged | class_local_06 | FLIPPED | 1 |
| M-git-lsfiles-route | 2.1 | ls-files --stage -z refuses a caller object-store route | w2_mutating | FLIPPED | 1 |
| M-git-lsfiles-argv | 2.1 | the exact argv is ls-files --stage -z | a6_a7, ls_files_stage_z, probe_01 | FLIPPED | 34 |
| M-target-git-dir (round 6) | 3.2 | cargo-target-v1 is off when the pinned git directory is, or is beneath, the root target | cargo_05, cargo_06 | FLIPPED | 2 |
| M-target-git-dir-beneath (round 6) | 3.2 | a git directory strictly beneath the root target also turns the policy off | cargo_06 | FLIPPED | 1 |
| M-sentinel-budget (round 6) | 3.3 | the object sentinel walks under the plan's entry budget | detector_07 | FLIPPED | 1 |

## 7. Exclusions and their mechanisms

1. **Container HTTP proxy:** workspace runs unset the proxy variables, as §7 and §13 direct.
2. **`cargo deny check`:** not run, because `cargo-deny` is not installed (`error: no such command: deny`). CI runs
   it. No dependency, feature, or `Cargo.lock` change was made.
3. **Native ext4 lane and macOS lane:** neither is available in this container, whose `/` and `/tmp` are overlayfs.
   - The planner is written for both targets. It uses only the 2B2b2a walker primitives, which are Linux- and
     macOS-only, and 2B2's helpers.
   - `separate_01` and `detector_05` use `--separate-git-dir`, for which Git writes the real path. On macOS that is
     `/private/var/…`, and the fixtures canonicalize their area first.
   - A case-insensitive APFS volume is untested here.
4. **The `ETXTBSY` fixture race.**
   - **Cause:** a test writes a script while another test thread forks. The child holds a writable duplicate until it
     execs, and exec of the script meanwhile fails `ExecutableFileBusy`.
   - **The CLI removal control** that failed the first turn's verify run has this race too. It fails 1 in 25 runs on the
     base commit (§0.1). The first repair turn's fix was outside the §8 paths and was reverted. The recommended fix for
     its owner is the same warm-up exec: that script's argument-less branch is `exit 1`, with no effect. It made 40 of
     40 runs pass.
   - **This slice's own fixture route** performs the same warm-up exec before use. Its scripts' default branch is
     `exit 97`, with no effect.
   - **2B2a's runner fixtures** (`custody_git_tests.rs`, `FixtureRoute::new`) are not changed.
     - The strays: 13 across about 400 matrix runs, never in a gate run.
     - Why unchanged: several of their scripts have effects when executed with any argument (for example
       `echo ready > ready` or `touch digest-marker` in the process's working directory), so a warm-up exec is not safe
       there, and the helper is outside this slice's §2.1 scope.
     - Recommended fix: have the fixture's bytes written by a child process (for example `/bin/cp` from a staging
       file), so the test process never holds a writable descriptor to the executed inode.

     This slice's controls spawn more processes in the same test binary, so they widen that window somewhat.
   - **`run_blocking_offloads_off_the_runtime_worker`**, a CLI control, is a wall-clock assertion that any
     whole-process stall of about 100 ms fails. It fails the same way on the base binary (§0.1). The recommended fix for
     its owner is to measure the ticker against the blocking closure's actual start and end, rather than against a
     fixed tick count in a fixed window.
   - **`compatibility_descriptor_handoff_validates_objects_before_close`**, a CLI control, failed once in 25 runs of the
     first turn's tree (`left: 1, right: -1`, a descriptor-number assertion). This slice does not touch it.
   - **The real-timeout control** (`runner_05`) uses an 8 s deadline, so a loaded run cannot time out `init` first.
5. **Windows:** the planner is `#[cfg(unix)]`. Nothing changes on Windows.

## 8. Interpretations the reviewer should check

1. **Fresh pins.**
   - **Mechanism.** "Each with its own freshly pinned root" is implemented as `PinnedDirectoryV1::open` of the retained
     pin's canonical path, then `identity().matches(retained)`, covering path, dev, ino, and btime.
   - **Why a path.** A fresh pin cannot be made from a retained descriptor without a path or new `unsafe`, because there
     is no descriptor-duplicating API.
   - **Scope.** This is a path-addressed *directory open*, verified to be the pinned directory. It is not a content
     read. Every content read (walks, index copies, dependency hashes, attributes, the gitfile, and alternates) goes
     through a descriptor.
   - **Failure.** A mismatch or a vanished root is `SourceRootDrift`.
2. **The census is not a walk.** It lists and states the git directory's top level through the **retained** pin.
   - Using a fresh pin there would have made the census a second, earlier guard for a replaced git directory.
   - Then the `M-barrier-gitdir` row could not discriminate its member, as §5.2 requires. With the retained pin, a
     replaced git directory is still caught later by the git-directory walks' fresh pins, after scratch writes.
     Only the barrier refuses before any write, which `barrier_02` proves.
3. **The gitfile check resolves lexically and exactly (repair turn, §0.2).** `resolve_gitfile_target` resolves the
   target the way Git does, relative to the directory holding the gitfile:
   - an absolute target as it is;
   - a relative target joined to the worktree's canonical path, each leading `..` removing one component of that
     symlink-free path.

   A `..` behind a component the target names (it could cross a symlink, as the decoy control shows) is refused. So is a
   `..` in an absolute target, and a `..` climbing past `/`. The result must equal the pinned git directory's canonical
   path. "By identity" rests on the barrier and the fresh pins, which prove that canonical path resolves to the pinned
   `(dev, ino)` before any write and at every git-directory walk. No path is stated and no symlink is followed.
   - A gitfile path that reaches the git directory only through a symlink fails closed. Git would follow it, but
     following it is a §9 stop condition.
4. **When `cargo-target-v1` applies.** It applies only when all of these hold, observed at plan start:
   - the root has regular `Cargo.toml` and `Cargo.lock` files;
   - the root `target` is a directory;
   - the pinned git directory is neither the root `target` nor beneath it, by canonical path (repair round 1, §0.3).

   Otherwise `reproducible_outputs` is `empty`, with no exclusion or dependency record. A file or symlink `target`, or
   a `target` that holds the git directory, is captured.
   - An exclusion of nothing would be a false claim. So would an exclusion of the git directory's own metadata.
   - The same `is_excludable_target` is used at plan start and in both selections, so `M-target-kind` removes one guard.
5. **A worktree park unresolves two rows:** the worktree (its walk stopped) and the class the park evidences.
   - `.gitmodules` and a nested `.git` evidence nested repositories.
   - The root `.git` evidences linked worktrees.
6. **A root `.git` that is neither the connector nor a regular gitfile parks, attributed to linked worktrees.** §3.2
   names only the connector and the gitfile. Including a foreign git directory as worktree content would hide it.
7. **A non-directory `worktrees`, `modules`, or `lfs` entry is `DependencyUnresolved`**, and an unreadable one is
   `ContentUnresolved`. An empty directory is owned by its class, which stays `empty`.
8. **The census budget.** The census lists the root under the plan's entry budget.
   - Over it, the ten census-dependent classes are `unresolved` (`ContentUnresolved`), and the git-directory walks and
     the probe are skipped. Those walks would refuse the same budget.
   - A census entry that vanishes before its stat is `IdentityChanged` for its owner.
   - Any other listing or stat failure is `Io`: an I/O failure on a pinned root.
9. **The equation reads the census.** For a git-directory class, the class-skip equation's "entries the other classes'
   tables own" is the census count of entries routed elsewhere. A top-level entry that appears or vanishes between the
   census and a walk is therefore `AccountingMismatch`, which fails closed (`skip_02`). The worktree's side is the
   excluded `target` only.
10. **Alternates evidence has two guards:**
    - the git directory's `objects/info/alternates`, read through the pin with no-follow;
    - the request's pinned chain being non-empty.

    They agree whenever the primary store is `<git_dir>/objects`. `alternates_01` separates them with a request whose
    primary store is elsewhere.
11. **The sentinel's frame header names `alternates_and_shared_stores`.** 2B2b1's frame refuses an `object_database`
    header. The sentinel's frame is directory-only and discarded, so no `object_database` frame is ever requested.
12. **An LFS-detector refusal** (budget, special file, drift) makes the **worktree** `unresolved` with the mapped
    reason, as §3.3 states. The lfs row is not additionally marked. The plan is unsealable either way.
13. **The stdout bound's "copied index bytes"** is the total of every copied file: `index` plus every `sharedindex.*`.
    A split index keeps its entries in the shared file.
14. **Index copy outcomes.** An open or read failure through the pin is `ContentUnresolved` (§4.5). A size or stat
    change during the copy is `IdentityChanged` (§4.3's drift row). The copy never writes past the opened size: one
    probe byte is read and never written.
15. **The `init --bare` exit status is checked before its re-measure.** 2B2 remeasures first. A failed init refuses
    the plan whatever it left, and remeasuring a missing directory would have reported `Io` instead of
    `ProbeInfrastructure`.
16. **Barrier refusals.** An alternates file that grew past its 2B2 bound since pinning (`SourceState`) is
    `SourceRootDrift`. Any other recheck failure is `Io`.
17. **"One mutation per recheck member."** The barrier's members are its four checks: repository, git directory,
    primary store, and alternate chain.
    - The primary and alternate members each cover two §5.2 rows, identity and alternates digest. 2B2's
      `PinnedObjectStoreV1::recheck` checks both in one call, and §9 forbids changing `custody_export.rs` behavior.
    - Each member's mutation turns red exactly its own rows, and the named greens prove the rest stay green.
18. **The plan carries more than §4.1 lists:** the gitlink evidence and the ledger's final charge, for the controls and
    for 2B2b2b2.
19. **The request is taken by reference.** Its sources are pinned by `CustodyCoverageSourcesV1::pin`, and a pinning
    failure is `Io`. A generation id the frame header refuses makes each walked class `ContentUnresolved`, a class-local
    frame refusal, rather than an error.
20. **Dependency binding.** Each dependency file is hashed at plan start under its opened stat. It must have the same
    stat after the worktree walk captured it, or `reproducible_outputs` is `IdentityChanged` and no record is emitted.
21. **No source recheck around the probe children.** Both runner callbacks are no-ops.
    - The probe never names the source: `GIT_DIR` is a single component under `work/`, with no object-store route.
    - The scratch was proved disjoint from the protected set.
    - The runner's own binary recheck and rooted spawn are unchanged.
22. **Receipts** are the encoder's summary (`frame_bytes`, `frame_sha256`) plus the walker's inventory digest, over an
    `io::sink()`.

**In-lane self-check.** Before the final matrix, the author re-read `custody_coverage.rs` against §2–§4. No separate
reviewer ran in this turn, so this is not the §10 review.

## 9. Owned paths and staged changes

Exactly the §8 paths changed. `bin/a2a-bridge/src/main.rs`, which the first repair turn had changed, is restored to
the base bytes, so the folded commit no longer touches it; `git diff --cached 3e12ff53 -- bin/` is empty. The harness,
the gates, the flake runs, and the structural RED output live under `.git/a2a-bridge/` and are not part of the diff.

```text
crates/bridge-core/src/custody_coverage.rs          (new)
crates/bridge-core/src/custody_coverage_tests.rs    (new)
crates/bridge-core/src/custody_git.rs               (modified: the §2.1 command only)
crates/bridge-core/src/custody_git_tests.rs         (modified: the §2.1 controls only)
crates/bridge-core/src/custody_export.rs            (modified: visibility and behavior-preserving extraction only)
crates/bridge-core/src/lib.rs                       (modified: the module declaration only)
docs/superpowers/reviews/2026-09-27-adr0041-slice2b2b2b1-implementation-handoff.md (new)
```

**Repair round 1** stages exactly three owned paths against `HEAD` (`92bc4a9f`, the committed slice, whose `bin/` is
already identical to the base):

```text
crates/bridge-core/src/custody_coverage.rs          (the git_dir_within_target conjunct and function)
crates/bridge-core/src/custody_coverage_tests.rs    (cargo_05, cargo_06, detector_07, and their fixture helpers)
docs/superpowers/reviews/2026-09-27-adr0041-slice2b2b2b1-implementation-handoff.md (this section, §0.3, and updates)
```

Immediately before staging, each source file's SHA-256 still equaled round 6's snapshot manifest (§0.3), and no
`pending.json` existed. `git diff --cached --check` exits 0. Against the base, the staged tree still differs in exactly
the seven §8 paths above. Nothing is committed, and no `.git/A2A_COMMIT_MSG` is written for this round.

## 10. What remains for the controller

1. The Sol implementation review's round 2, the last under the §10 two-round cap. It should check that §0.3 closes
   round 1's findings:
   - the `target` containment conjunct closes the `cargo-target-v1` blocker, including its canonical-path
     interpretation;
   - `detector_07` and `M-sentinel-budget` close the sentinel-budget SMELL.
2. The inherited CLI flakes (§0.1, §7, item 4) belong to the `a2a-bridge` CLI's owner. A verify run can still hit them,
   as it can on the base commit.
3. The controller's macOS lane: the §7 gates on macOS.
4. CI: native ext4, the Windows compile, `cargo deny`, and coverage.
5. 2B2b2b2: capability binding and staged-frame export against `CustodyCoveragePlanV1`, as the design notes require.
6. The parent plan, the roadmap, and the planning handoff, which the controller alone updates.
