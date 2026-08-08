# S4 clone reaper — dual-lens review record

Date: 2026-08-08. Artifact: `feat/s4-clone-reaper` @ `d48079d` → repaired `3d2e081` (base `01f2aa1`).
Lenses: Opus/high senior-lead posture; gpt-5.6-sol/high via the bridge's `code-review` workflow. One
declared round + one combined bounded repair.

## Verdicts

- **Opus senior-lead: REVISE** — 1 BLOCKER (containment proves HEAD, deletion takes every ref — verified
  against two production shapes: rogue agent branches surviving `restore_branch` by design, and unlanded
  merge artifacts), 1 WRONG (evidence symlink silent-skip with false receipt claims), 6 SMELLs. Endorsed:
  the A4 `remove_dir_all` deferral **even on the unrecoverable class**, exhaustion-sentinel and
  ancestor-operand correctness, compiler-enforced `ItemSource` migration, S3 semantics unchanged.
- **Sol/high: REJECT** — 8 blocker-graded WRONGs, 2 DEFERs. Beyond the converged findings, Sol uniquely
  established: `assume-unchanged`/`skip-worktree` index flags silence porcelain over modified bytes;
  clean initialized submodules carry sole-copy objects in `.git/modules`; bare `rev-parse main` can
  resolve a TAG named main (mutation control observed a real deletion pre-fix); discarded fsync errors on
  evidence/receipt barriers; false descendant projection on `Partial`/`Unknown` outcomes.

## Posture note

On regenerable classes (S3) the strict lens over-graded and the senior-lead lens adjudicated correctly;
on the unrecoverable class the strict lens found five real mechanisms the judgment lens missed. Standing
conclusion: dual-lens is mandatory for slices that can destroy unique bytes; senior-lead-only suffices
elsewhere.

## Combined repair (ten items, all defect-red → green at `3d2e081`)

F1 ref sweep (refs/heads/* + tags + stash; tip must be HEAD, ancestor-of-HEAD, or independently on
source main; dangling objects documented as deterministic recomputations); F2 index-flag gate
(`ls-files -v` non-`H` parks; sparse parks); F3 ignored-entry disposability now reuses S3 provenance on
the LAST path component (fixes nested-crate friction and the trailing-slash fail-open in one rule); F4
initialized submodules park; F5 evidence ambiguity parks in both directions (source symlinks/specials,
symlinked `.receipts`, destination identity) with absent-vs-ambiguous distinguished and
`A2A_TASK.md`/`A2A_COMMIT_MSG` added to the preserved set; F6 every durability barrier propagates and
parks pre-removal; F7 main resolves only `refs/heads/main^{commit}` → `refs/heads/master^{commit}` →
source HEAD's branch, full ref + OID in the receipt; F8 non-clean outcomes restat descendants and persist
a presence map; F9 `ReapItem.source` serialized (kebab-case = label = docs); F10 checkpoint-vs-origin
cross-check parks repointed origins, dry-run labels preservation gates "NOT exercised" and runs the
non-mutating structural preflight.

## Carried deferrals

Descriptor-relative recursive removal (A4 — both lenses endorse deferral; the S4 boundary duplication of
S3's verify-then-act block is the A4 extraction's shape); FoldReceipt PR-number (unknowable offline;
`matched_commit` + full ref is the durable identity); rename-porcelain/dirty-submodule regressions (safe
by construction — every non-`!!` line refuses).

## Gate evidence at `3d2e081` (claimed by implementor, orchestrator reruns the full gate at fold)

fmt/clippy/diff-check/hygiene clean; focused 158/0; whole-bin 1207/0/11 twice (task-#9 flake quiet); live
fixture: 5 new park classes exercised verbatim; real reap deleted exactly the 3 planned clones, preserved
all four evidence artifacts, upstream and 12 parked clones untouched.
