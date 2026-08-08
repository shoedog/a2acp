# S4b owner disposition + chained-origin resolution — dual-lens review record

Date: 2026-08-08. Artifact: `feat/s4b-owner-disposition` @ `ff77ab14` → repaired `c3f1c1c5` (base
`a01e7891`). Lenses: Opus/high senior-lead; gpt-5.6-sol/high via the bridge. One round + one combined
repair.

## Verdicts and the ref-sweep ruling

- **Opus: REVISE (no blocker)** — the license cannot leak to unlisted clones (exact BTreeSet match over a
  directory-name-proven run id) and cannot waive any non-replaced gate; required the ref INVENTORY on
  licensed receipts and the terminal-root git-repo check.
- **Sol: REJECT (4 blockers)** — license-file stat→read substitution window; checkpoint-less hops
  accepting an uncorroborated terminal root; `source_repo: None` over-broadly dropping a removal-guard
  clause; and the decisive one: parent-first deletion **permanently** strands chained children (the
  implementor's "second invocation resolves it" disclosure was false, and a test helper had filtered the
  shape away).
- **Both lenses concurred on the flagged question:** run-scoped abandonment is the correct license
  semantics — the ref sweep does NOT gate under license (it mechanically cannot serve the parent-reaped
  population the license exists for; dirty bytes park because they are indescribable, committed refs are
  describable) — but the ref inventory `{refname, oid, is_head, is_ancestor_of_head}` is recorded on the
  receipt and gate text so the loss is informed and auditable.

## Adjudication notes

Sol's substitution blocker was downgraded to hardening (a same-user substituting process already holds
direct unlink authority; the single-fd `O_NOFOLLOW` load removes the window anyway, ~5 lines, honestly
reported as untestable-behaviorally). Sol's stranding blocker was upheld in full and fixed by phase
separation: every candidate's origin chain resolves at admission, verdicts rest on the cached terminal
root (outside the scan root, re-verified by identity at the boundary), so in-invocation deletion order is
irrelevant; chains broken by PRIOR invocations do not self-heal and the F2 license is the designed
recovery.

## Repair highlights (`c3f1c1c5`, all defect-red first; 82/0 module, whole-bin 1234/0/11 ×2)

Terminal root must pass `looks_like_git_repo` AND source probes run under `GIT_CEILING_DIRECTORIES` set
to the root's PARENT — the mutation round proved ceiling=start-dir is a silent no-op (proper-ancestor
semantics), and an empty-`.git` decoy discriminates why both halves are needed. Checkpoint-less chains
are corroborated by the terminal root containing the clone's recorded base (or lineage root).
`source_repo: None` now only for absent-origin (parent reaped); an existing origin always feeds the
removal-guard clause. Listed+non-listed side-by-side, disclaimer-string, relative/symlinked-origin, and
`removal_guard(None)` test gaps closed.
