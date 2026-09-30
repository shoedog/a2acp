I’ll bind the exact checkout and owned documentation diff first, then read the revision-2 task, its parent design and plan, the raw round-1 review and resolution note, and every cited implementation seam. I’ll keep this strictly source-inspection only and treat all supplied GREEN claims as unverified receipts.The checkout and range match exactly: HEAD is `a25ddb01…`, and the base-to-HEAD change is documentation-only across six files. I found no working-tree modifications in the status output. I’m now reading the complete task and the prior review record, then I’ll cross-check each promised contract against the parent design/plan and production types.Revision 2 directly addresses all four round-1 items in the text: it makes empty format provenance explicitly unauthenticated, aliases final HOME/XDG to `.git`, replaces path remeasurement with a descriptor-relative census, and moves active-frame prevalidation ahead of `repository/`. Those repairs are coherent at the design level. I’m now testing whether the existing runner, manifest/ref vocabulary, frame roles, ledger, and retained-descriptor APIs can actually implement the revised requirements without reopening paths or creating unbounded states.The existing seams support the four stated repairs, including `.git` aliasing under the runner’s single-root model and a bounded descriptor-relative census through retained pins. One deeper interaction remains under review: `InitBare` itself creates its target by name, while revision 2 simultaneously promises collision refusal and no mutation of any preexisting object. I’m tracing the runner/exporter’s exact initialization sequence and mutation controls to determine whether the current command vocabulary can close that creation race.The checkout is clean at `a25ddb01a9322f5994604f51c05c0378efe64965`; the range from `5d2a82c457e626b2a108063e1de6b13b94e97f92` is documentation-only. I inspected the complete task, parent design and plan, round-1 review and resolution, and the relevant restore, exporter, Git runner, filesystem, frame, coverage, seal, capsule, and test seams. I did not edit, build, test, install, use network/providers, spawn agents, or credit prior GREEN evidence.

The four round-1 blockers are repaired:

- Empty databases now use explicit caller-selected format with unauthenticated provenance, while Captured databases retain pack-backed format proof.
- Final HOME/XDG/GIT_DIR can all name `.git`; the runner accepts identical component names and disables system/global configuration.
- The restore-specific census can be implemented with the existing retained-directory listing, no-follow metadata, directory-pin, and regular-file APIs.
- Active-frame type and path validation now precedes every `repository/` write.

## WRONG

None.

## SMELL

### 1. “Portable-equivalent” has no exact definition

**Confidence:** High  
**Disposition:** DEFER

**Trigger/reachability:** A valid Linux-produced frame can contain byte-distinct names such as `refs/heads/A` and `refs/heads/a`, Unicode normalization variants, backslashes, or non-UTF-8 components. The frame codec deliberately accepts these and assigns portability decisions to restore ([custody_frame.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_frame.rs:316)). The task requires rejecting “portable-equivalent spelling collisions” without defining the equivalence relation ([task](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:99)).

**Risk:** Implementations can choose materially different acceptance sets while each appears to satisfy the wording. No incorrect result is presently demonstrated because conservative refusal remains available.

**Bounded fix:** Define the exact component-key transformation or reference a canonical existing function, including treatment of ASCII case, Unicode, backslash, trailing dots/spaces, and non-UTF-8 bytes.

**Regression:** A table of name pairs with explicit collide/distinct expectations, covering ASCII case, NFC/NFD, backslash, and non-UTF-8.

**Evidence that would collapse it:** An authoritative existing equivalence function or table that the task explicitly adopts.

### 2. Bootstrap create-new behavior lacks a direct regression

**Confidence:** Medium-high  
**Disposition:** DEFER

**Trigger/reachability:** After Phase V, an operator tool places an empty `.restore-work/git-bootstrap` directory before G begins. The global contract requires any preexisting intended slot to refuse, but Task 2 explicitly creates only HOME/XDG before invoking path-addressed `InitBare` ([task](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:113)). The existing command passes the target as a component to `git init` ([custody_git.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_git.rs:101)), and the exporter currently relies on Git to create its init targets ([custody_export.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_export.rs:1837)).

**Risk:** An implementor following the existing exporter pattern could reuse the preexisting empty directory rather than prove that this invocation created and pinned it. The global create-new rule still forbids that implementation, so this is a regression gap rather than a demonstrated contract contradiction.

**Bounded fix:** State explicitly that G reserves, creates new, and pins `git-bootstrap` before `InitBare`, counting that directory within `GitDirectoryBudgetV1::for_init`’s existing nine-entry reservation.

**Regression:** Preplant an empty directory, regular file, and symlink at `git-bootstrap`; each must refuse before any Git child starts and remain untouched. Include the unplanted success control.

**Evidence that would collapse it:** An explicit task requirement and test proving the bootstrap target itself is created new and pinned before `InitBare`.

VERDICT: APPROVE
SUMMARY: wrong=0 smell=2 blockers=0 round=2/2

