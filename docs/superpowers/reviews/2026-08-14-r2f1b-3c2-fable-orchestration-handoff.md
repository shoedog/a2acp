# R2f1b 3c2 A2-G Fable orchestration handoff

Date: 2026-08-14

Role: You are Fable, the primary orchestrator for completing R2f1b sub-slice
3c2 in:

`/Users/wesleyjinks/code/a2a-bridge`

## Objective

Orchestrate the remaining nine tasks of the owner-selected ten-task 3c2 API
request-flight plan:

`A2 -> A3 -> A4 -> B -> C -> D -> E -> F -> G`

A1 is already closure-approved at exact commit:

`5cbeea1ed882afe448d3825984af9a3ed74bcb58`

It is retained in:

`/Users/wesleyjinks/code/.a2a-implement/impl-77617-f18mbkc5`

A1 is not integrated. A2 must start from that exact commit. Do not reconstruct,
restart, reimplement, or substitute another A1 lineage.

## Read first, in order

1. `/Users/wesleyjinks/code/a2a-bridge/AGENTS.md`
2. `/Users/wesleyjinks/code/a2a-bridge/skills/a2a-bridge-operator/SKILL.md`
3. `docs/reliability-execution-roadmap.md`
4. `docs/superpowers/plans/2026-08-12-r2f1b-3c2-salvage-redesign.md`
5. `docs/superpowers/reviews/2026-08-12-r2f1b-3c2-redesign-adjudication.md`
6. `docs/superpowers/reviews/2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md`
7. `docs/superpowers/reviews/2026-08-14-r2f1b-3c2-task-a1-owner-extension-adjudication.md`
8. `docs/superpowers/reviews/2026-08-14-r2f1b-3c2-task-a1-owner-extension-sol-closure.md`
9. The current feature handoff:
   `/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Binding identities

- Landed slice-3 base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`.
- Planning decision checkpoint: `334201aa957fedd4c5c50e90f3c99ddfc0db231f`.
- Current feature-handoff checkpoint: `2c6505ea`.
- Closure-approved A1 input: `5cbeea1ed882afe448d3825984af9a3ed74bcb58`.
- The preserved rejected 3c2 feature artifact must not be folded or used as
  accepted delivery.

Before dispatching anything, perform a read-only identity preflight:

- verify every named checkout, branch, HEAD, candidate, and parent relationship;
- verify the retained A1 clone is clean and exactly at `5cbeea1e`;
- verify current `origin/main` against the recorded landed base;
- distinguish existing user-owned untracked files from lane artifacts; and
- if any identity has drifted, stop and report the exact mismatch rather than
  silently rebasing or recreating the candidate.

## How this plan fits into R2f1b

Slices 3s, 3a, 3b1, 3b2, and 3c1 are already folded on green main. They
established truthful failure settlement and safe ownership of shared cleanup,
processes, wrappers, and containers.

3c2 extends that protection to individual API provider requests. A-G contains
ten implementation tasks because A is split into A1-A4:

- A1-A4: safe storage and ownership of the request record;
- B: durable request admission, numbering, capacity, and retirement;
- C: restart recovery and reliable final-result publication;
- D: one owned request lifecycle with accurate `not sent` versus `may have been
  accepted` outcomes;
- E: truthful API cleanup;
- F: migrate the actual API send path onto the new request mechanism; and
- G: allow retry only after cleanup is proven complete.

Completing A1-A4/B-G completes 3c2, not all of R2f1b. Later 3d still owns
preparation-flight and candidate settlement. Production V3 remains unarmed
throughout 3c2.

## Execution method

Reuse the orchestration method that converged 3s through 3c1:

1. Freeze the exact predecessor commit before each dispatch. Never use a branch
   name or moving ref as task input.
2. Run the tasks strictly sequentially. Do not parallelize A2 through G.
3. Treat the salvage plan as binding for every task's owned paths, required
   behavior, red regressions, focused gates, changed-line limits,
   stop/split conditions, and common full-repository gate.
4. Each task must leave a green tree, refresh the lane handoff, and produce one
   durable commit before its successor is dispatched.
5. Declare the review cap before every task:
   - one independent implementation review;
   - if rejected with a closed, enumerable population, one targeted repair on
     the same artifact and one closure review;
   - at the cap, classify before acting;
   - shrinking, non-repeating findings may receive only a disclosed operator
     extension;
   - repeated or open-class findings park for design; and
   - never restart with a fresh implementation.
6. Findings must obey `AGENTS.md`:
   - WRONG before SMELL;
   - every WRONG names the reachable state/input and incorrect result;
   - same-environment base controls are required before attributing gate
     failures; and
   - failed or malformed probes are inadmissible.
7. Preserve every stable artifact, review, adjudication, gate receipt, roadmap
   update, and handoff. Do not leave important evidence as an untracked single
   copy.

## Task sequence

- A2 begins only from exact A1 commit `5cbeea1e`.
- A3 begins only from the accepted A2 commit.
- A4 begins only from the accepted A3 commit.
- B begins only from accepted A4.
- C begins only from accepted B.
- D begins only from accepted C.
- E begins only from accepted D.
- F begins only from accepted E.
- G begins only from accepted F.

Use the exact task contracts and caps in:

`docs/superpowers/plans/2026-08-12-r2f1b-3c2-salvage-redesign.md`

Specifically, follow its `Compile-correct implementation tasks` section. Do not
broaden Task A into the shared generation journal, worktree custody,
`local_file`, reapers, recursive deletion, or general filesystem transaction
machinery.

## Gates and closure

After every task, run the exact common gate in the salvage plan. Record command,
exit status, totals, exclusions, and exact HEAD. A red gate blocks the successor.

After G:

1. Run one aggregate dual-lens round on the exact combined diff:
   - Sol/xhigh for concurrency and ownership correctness; and
   - Fable/Opus xhigh for release, compatibility, rollback, and cross-slice
     authority.
2. Give each lens one completed pass with no automatic retry after prompt
   acceptance.
3. Operator-adjudicate both reports against production callers, persistence,
   wrappers, cleanup, and final retry decisions.
4. Rerun the complete gate on the exact final candidate.
5. Fold only if the fold tree is byte-identical to the gated tree.
6. Land or push only through the existing controlled integration boundary and
   only when that authority is present. Otherwise stop with a land-ready
   handoff.
7. Reconcile the roadmap, feature handoff, planning records, and final evidence
   in the same turn.
8. Do not declare 3c2 complete until post-landing CI is green across the required
   lanes.

## Non-scope

This handoff does not authorize:

- OpenRouter or OpenCode implementation;
- a live or billable provider turn;
- compatibility execution;
- production V3 activation;
- creation of a production request-journal root;
- 3d implementation;
- automatic deadlines;
- release, deployment, or running-operator mutation.

Read-only OpenCode/OpenRouter discovery may occur independently, but it is not
part of this lane and must not modify the API request path or disturb the
sequential A2-G lineage.

The two-field cleanup carry-forward remains binding:

```rust
CleanupReportV1 {
    result: inner_disposition,
    checkout: checkout_disposition,
}
```

Only `Complete + Complete` may become `Complete`.

## Start condition

Start by reporting the identity preflight, the declared A2 implementation and
review cap, the exact A2 input, and the orchestration record location. Then
dispatch A2 and persist each stable point as you proceed.
