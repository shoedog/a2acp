I’m binding the exact checkout, base, owned documentation diff, and relevant prior 2B3a custody context first. Then I’ll read the task, parent design/plan, implementation seams, and tests source-only; I will not execute or modify anything.The checkout is clean at the required `b199bfd…`; the base resolves correctly and the four owned paths are documentation only. I’m now mapping the contract sections and source entry points so the subsequent reads cover the full state machine rather than only the new task wording.The specification’s core shape is now clear. The main correctness pressure points are implementability against existing private fields, exact pack and frame semantics, the Git runner’s root/environment model, and whether the proposed final validation can occur without reopening names or activating archived behavior. I’m tracing those seams next, including the current 2B3a tests and exporter parsers.Two specification conflicts are now constructible: the valid empty-object capsule has no pack although the task requires exactly one and says to index it, and the final Git-runner invocation can only place its single-component HOME/XDG beside `.git` inside `repository/`, leaving extra worktree entries that later exact restoration cannot remove. I’m checking whether existing APIs offer a path around either conflict and auditing the mandated budget/measurement helper for no-follow and final-chain guarantees before fixing the count.The checkout and documentation-only scope are bound to the requested revisions. I have two direct contract contradictions and am now finishing the source-level reachability check for the symlink and filesystem-budget seams before fixing the finding count. No test or prior-run GREEN evidence is being credited.The remaining source checks expose two more contract-level failures: the mandated remeasurement helper follows renamed symlinks, and frame-type rejection is ordered after final object-store creation despite the spec’s “before active materialization” promise. I am calibrating these as separate blockers because each has a distinct trigger and bounded repair.The requested checkout is clean at `b199bfd0f44f88889a564dc4d1c352bbc7ec717b`, based on `5d2a82c457e626b2a108063e1de6b13b94e97f92`. The owned diff is documentation-only. I reviewed the complete task, parent design and plan, current restore/export/Git/frame/coverage/seal/filesystem seams, and relevant tests. I did not build, test, edit, write, use network/providers, or credit supplied or historical GREEN evidence.

## WRONG

### 1. Empty repositories have no pack, making the required admission contract impossible

**Classification:** WRONG — BLOCKER  
**Confidence:** High

**Input/state:** A valid manifest with `object_database=Empty` and an empty object inventory.

**Incorrect result or unreachable promise:** The task requires all of the following:

- an empty inventory with the correct supplied format succeeds;
- an absent pack refuses;
- exactly one pack is identified;
- strict indexing and pack-format verification distinguish the correct SHA-1/SHA-256 choice.

The existing capsule layout deliberately emits no `GitObjectPack` for this valid state. It emits only the three control artifacts when all classes are empty. With no objects and no pack, no authenticated input distinguishes an empty SHA-1 repository from an empty SHA-256 repository. Therefore the “wrong object format refuses” promise cannot be implemented while the “correct format works” and “absent pack refuses” requirements remain.

This conflicts directly between [the task’s API and empty-inventory requirements](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:64) and [the existing derived capsule layout](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_capsule.rs:201).

**Single-operator reachability:** This is the ordinary exported state of a valid empty repository. It requires no race or hostile mutation.

**Bounded fix:** Amend the owning capsule schema/design to authenticate object format independently of inventory, or include a format-bearing empty pack for an empty object database. Then specify separate zero-object admission and closure behavior. The present task’s prohibition on wire-format changes means implementation must stop rather than infer correctness from the caller.

**Failing regression:** Export an empty SHA-256 repository, restore it once with SHA-256 and once with SHA-1, and require the first to succeed and the second to refuse based on sealed evidence. Also assert the expected empty-capsule artifact layout.

**Evidence that would change the finding:** An authenticated field or sealed artifact already present in an empty capsule that identifies SHA-1 versus SHA-256. The current layout shows none.

### 2. Final HOME/XDG directories necessarily corrupt or collide with the restored worktree

**Classification:** WRONG — BLOCKER  
**Confidence:** High

**Input/state:** Any source worktree, including an empty worktree or one containing the fixed names chosen for phase G’s final HOME/XDG directories.

**Incorrect result:** The task requires phase G to create pinned empty HOME and XDG children directly under `repository/`, preserve all created bytes, and perform no cleanup. Phase P subsequently restores the worktree into that same directory and skips only the `.git` connector.

For an empty source worktree, the final repository gains two source-foreign directories. If the source contains either chosen name, create-new worktree restoration collides with the phase-G directory. Either result violates byte-exact worktree reconstruction.

The runner forces HOME, XDG and GIT_DIR to be single components relative to one root: [GitRootNamesV1](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_git.rs:201), [environment construction](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_git.rs:1151), and [component validation](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_git.rs:1581). The conflicting placement is required by [Task 3](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:128), while [Phase P restores directly into `repository/`](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-09-27-adr0041-slice2b3-restore-design.md:98).

**Single-operator reachability:** Every successful phase-G execution creates these directories. No concurrency is involved.

**Bounded fix:** Revise the runner contract so final commands can use independently retained HOME/XDG directories under `.restore-work` while GIT_DIR addresses final `.git`, or define and prove another placement that introduces no worktree entries. That requires an explicitly reviewed runner amendment; the task currently declares `custody_git.rs` unchanged.

**Failing regression:** Restore both an empty worktree and a worktree containing the proposed HOME/XDG names. After Phase P, assert that `repository/` contains exactly the source worktree plus `.git`, with no collision or extra phase-G directories.

**Evidence that would change the finding:** An existing runner mode that roots HOME/XDG outside `repository/` while addressing `repository/.git`, or an authorized later phase that removes or adopts these directories without weakening create-new and no-deletion guarantees. Neither exists.

### 3. The mandated Git-directory remeasurement follows symlinks

**Classification:** WRONG — BLOCKER  
**Confidence:** High

**Input/state:** At the task’s planted-link mutation seam, replace `git-bootstrap/HEAD` after `InitBare` with a symlink to an external regular file containing a valid HEAD value and fitting within the reservation.

**Incorrect result:** Task 2 mandates `remeasure_git_directory_in` after Git children, while the global contract says links are never followed. That helper converts the retained pin back into a path, recursively uses `std::fs::read_dir`, and calls `DirEntry::metadata`, which follows symlinks. A symlink named `HEAD` is also accepted as an expected file because `verify_git_writes` compares only the resulting relative filename and byte total. The restore can therefore read an external target instead of issuing the required link refusal.

See [the mandated reuse](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:101), [path-based measurement](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_export.rs:2533), and [the absence of an expected-type or missing-file check](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_export.rs:2618).

**Single-operator reachability:** The task explicitly requires planted-link and callback mutation regressions before the final gate. This is within the stated model; only hostile mutation after the final check is excluded.

**Bounded fix:** Revise the task to authorize a descriptor-relative, no-follow bounded remeasurement routine, either by hardening the exporter helper with exporter-preservation coverage or by adding a restore-specific helper using `PinnedDirectoryV1`. Remove the unconditional requirement to reuse the current path walker.

**Failing regression:** Replace bootstrap `HEAD` with a symlink during the post-`InitBare` callback. Require a typed symlink/identity refusal before `IndexPackStrictStdin` starts, and prove the external target was not opened or measured. The current helper instead treats it as the expected `HEAD` file.

**Evidence that would change the finding:** Proof that `DirEntry::metadata` and the recursive `read_dir(entry.path())` calls cannot encounter a symlink at every declared mutation seam. The current task explicitly requires testing such a seam.

### 4. Git-frame symlinks are rejected only after active object materialization

**Classification:** WRONG — BLOCKER  
**Confidence:** High

**Input/state:** A fully valid `RefsAndHead` frame containing a filesystem symlink under `refs/`.

**Incorrect result:** The global task contract says Git-frame symlinks refuse before active materialization. Current framing and export support such entries: the walker emits symlink records and the Phase V decoder accepts and verifies them. Yet the task orders creation and population of `repository/.git/objects` in Task 3, then performs active-frame path/type prevalidation and symlink refusal in Task 4. The refusal therefore occurs only after active materialization has begun.

See [the before-active-materialization promise](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:54), [Task 3’s active object-store creation](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:118), and [Task 4’s later prevalidation](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:134). The source seams permit the input through [the walker](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_walk.rs:644) and [frame decoder](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_frame.rs:864).

**Single-operator reachability:** A source `.git/refs/**` symlink is captured descriptor-relatively into a valid frame. No concurrent mutation is required.

**Bounded fix:** Move complete active-frame path/type/conflict/collision prevalidation ahead of Task 3’s first `repository/` write. Bootstrap work beneath `.restore-work` may remain earlier because it is isolated rather than active.

**Failing regression:** Supply a valid capsule whose `RefsAndHead` frame contains a symlink. Require refusal with `active_materialization_began=false` and assert that `repository/` does not exist. Bootstrap bytes may remain under `.restore-work`.

**Evidence that would change the finding:** A prior gate that records and refuses frame entry types before Task 3. Phase V currently verifies framing integrity but deliberately accepts symlink records.

## SMELL

None. The raw-ref requirements cover loose-over-packed precedence, symbolic and unborn states, cycles, peeled rows, extra refs, and inventory closure sufficiently to admit a bounded implementation. The current Phase V retained fields and final gate also provide the required pin and ordering seams. These source-level observations do not offset the four blockers.

VERDICT: REJECT  
SUMMARY: wrong=4 smell=0 blockers=4 round=1/2

