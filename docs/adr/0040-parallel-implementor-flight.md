# ADR-0040 — Parallel implementor flight and linear integration

**Date:** 2026-07-30
**Status:** Accepted

**Builds on:** ADR-0025 (concurrent run isolation), ADR-0026 (durable implement checkpoints), and
ADR-0027 (operator-authored merge hand-off).

## Context

Several `a2a-bridge implement` commands can already execute concurrently. Each run owns a distinct clone,
container identity, and liveness lease, so work in flight does not share a worktree or container. The missing
process mechanism is integration: ADR-0027 Mode A admits a run only while the destination branch still equals
that run's immutable `base_commit`. When two approved siblings start from one base, the first can land and the
second must be rerun even when their changes are independent.

Rerunning an implementor discards reviewed work and spends another agent turn. Blindly cherry-picking in the
operator checkout would instead abandon the bridge's no-touch guarantee and its compare-and-swap push boundary.
Parallel work also needs an ownership protocol: concurrent clones make collisions possible to recover from, but
do not make overlapping task briefs safe.

## Decision

### 1. Flight protocol

A parallel implementation flight has one operator and one frozen base commit.

1. Before dispatch, the operator writes independently testable task specs and an ownership ledger. Every changed
   path or named seam has one implementor. Shared manifests, roadmaps, generated files, and cross-cutting cleanup
   belong to a designated integration task unless the specs state a non-overlapping edit.
2. Every sibling starts from the same immutable `--base-ref <sha>`. Siblings may run concurrently with one shared
   bridge config because ADR-0025 isolates their clones and runtime identities.
3. Siblings do not use `implement --merge`. The operator waits for terminal checkpoints, inspects every Approved
   result, and parks any rejected or overlapping result for a bounded repair or ownership decision.
4. Approved siblings are integrated one at a time in declared dependency order. The first may use exact-base
   merge. Each sibling uses `a2a-bridge merge <id> --onto <target> --integrate-current`.
5. After each landing, the target lease remains the concurrency compare-and-swap. The final combined target gets
   the aggregate build/test gate and a review of the cumulative diff from the frozen base. Individual green runs
   are not evidence that their composition is green.

The canonical reliability roadmap remains the single program cursor. A flight ledger is evidence about work in
progress, not a second roadmap or authority to merge.

### 2. Explicit current-target integration

`merge --integrate-current` is an opt-in extension of ADR-0027. Default `merge` and `implement --merge` retain
their exact-base behavior.

For the explicit mode, the bridge:

1. performs the existing terminal-checkpoint, clone-HEAD, clean-worktree, history, identity, and checked-out-target
   preflights;
2. reads and fetches the current destination commit into the quarantine clone;
3. requires the run's `base_commit` to be an ancestor of both the approved run commit and the fetched destination;
4. uses `git merge-tree --write-tree --merge-base=<base>` to apply the approved run's tree delta to the fetched
   destination without changing either checkout;
5. creates one operator-authored commit whose parent is the fetched destination and whose tree is the clean merged
   tree; and
6. pushes it with `--force-with-lease=<target>:<fetched-destination>`.

When that merge tree already equals the fetched destination tree, step 5 creates no empty commit and step 6 uses a
verify-only `git update-ref --stdin` transaction instead. The transaction locks and compares the destination ref at
one linearization point; a same-value push is insufficient because Git may send no update command after advertising
the destination.

This produces a linear target and preserves the reviewed run commit as the immutable delta source. It is not an
agent retry, prompt replay, textual patch application, or automatic conflict resolution.

If the destination is not a descendant of the frozen base, the tree merge conflicts, the target is checked out,
or any preflight fails, the bridge makes no destination update and retains the clone. If the destination moves
after it is fetched, the push lease refuses; the operator may rerun the same non-agent integration command, which
recomputes against the new destination. No failed integration automatically spends another agent turn.

### 3. One operation lock per implement run

`implement --resume` and `merge` acquire the same non-blocking advisory lock at
`.a2a-implement/.operation-locks/<id>.lock`, outside the clone that a successful merge reaps. Two operations on one
run therefore cannot reconcile, create integration objects, push, or reap the same clone concurrently, and clone
reaping cannot unlink the held lock namespace. Operation-lock paths persist after release, so a contender that
opened the path before release and a later opener cannot acquire locks on different inodes. Different run IDs use
different lock files and remain parallel.
The destination lease, not this per-run lock, remains the cross-run atomicity boundary.

## Consequences

- Multiple implementors can keep work in flight and retain useful Approved siblings after the first target move.
- Conflict handling is deliberately conservative and operator-visible. The bridge does not synthesize conflict
  fixes or silently change task ownership.
- Integration requires a Git version that supports `merge-tree --write-tree` with an explicit merge base. A
  missing capability is a preflight failure with the clone retained.
- Parallelism is bounded by operator-authored decomposition. Automatic task decomposition, a batch launcher, and
  automatic aggregate review are outside this increment.
- Successful integration still reaps only the exact bridge-owned clone through ADR-0027's guarded deletion.

## Alternatives considered

- **Rerun every stale sibling from the new target.** Safe, but wastes reviewed work and billable turns even for
  disjoint changes.
- **Push each result to a staging branch.** Preserves commits but leaves composition, identity normalization, and
  final target CAS as manual steps.
- **Cherry-pick or rebase in the operator checkout.** Mutates operator state and loses the existing no-touch
  guarantee.
- **Automatic conflict-repair turns.** Expands authority and billable scope after a rejected composition; conflicts
  instead stop for a new bounded decision.
