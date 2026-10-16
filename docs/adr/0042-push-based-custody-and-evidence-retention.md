# ADR-0042 — Push-based custody and configurable evidence retention

**Date:** 2026-10-15
**Status:** Accepted (owner, 2026-10-15)

**Supersedes:** the custody, capsule, restore, promotion and verification design of ADR-0041 (§§3–15, §17).
ADR-0041 §2's findings remain the requirements this ADR must meet.

**Builds on:** ADR-0025 (concurrent run isolation), ADR-0026 (implement checkpoints), ADR-0027 (operator-authored
merge), and ADR-0040 (parallel implementor flight).

## Context

An implement clone holds two things worth keeping: the run's commits and the run's evidence (task spec, plans,
reviews, transcripts, verify logs). ADR-0041 kept both by sealing a self-contained capsule per clone, then
restoring it, promoting it off the laptop, and verifying it independently, before allowing deletion. Stages 1–2 of
that plan are merged but not wired into any command. Stage 3, the only stage that helps after a lost laptop, is
still blocked on four owner decisions.

The owner reviewed that design on 2026-10-15 and ruled it out of proportion to the goal:

- **Local capsules do not help after a lost laptop.** Only an off-laptop copy does.
- **Full per-clone capsules store the repository's history once per clone.** A hundred clones store the same
  history a hundred times.
- **Git does not sync reflog-only or unreachable objects, stashes, in-progress merge state, or `.git/config` and
  hooks to a remote, and developers do not expect to get them back.** The usual way to recover that state is a
  whole-disk backup (Time Machine, Backblaze, restic, borg). Recovering it is out of scope here.
- **Almost nobody reads old run evidence.** Keep a small, configurable amount locally by default, and let users
  choose more.

## Decision

### 1. Commits are protected by pushing them

At each stable point, the bridge commits the clone's state and pushes the run branch:

- after each implement or fix attempt that changes the tree;
- before each review;
- before merge.

A dirty or interrupted clone gets a WIP commit first, so nothing tracked or untracked-but-not-ignored is left only
on disk. The bridge host does the push. Agents and containers never receive push credentials.

Configuration (`[custody]`):

| Key | Default | Meaning |
|---|---|---|
| `push` | `true` | Push the run branch at stable points |
| `remote` | `origin` | Remote to push to; may name a private mirror |
| `ref_prefix` | `wip/` | Branch prefix; the run branch is `<prefix><run-id>` |
| `delete_remote_on_merge` | `false` | Delete the run branch after a successful merge |

The pushed branch keeps the original commits. That is what makes `merge`'s re-authored commits safe: the originals
remain on the remote.

On a public repository, the run branch is public. Set `remote` to a private mirror if that is not acceptable.

### 2. Evidence is kept by a configurable policy

At merge, or at the terminal state of an unmerged run, the bridge writes one evidence archive per run.

Configuration (`[evidence]`):

| Key | Default | Meaning |
|---|---|---|
| `store` | `local` | `local` is the only accepted value |
| `path` | platform state dir, `evidence/<repo-key>/` | Where `local` archives are written |
| `include` | `["spec", "final_review_summary"]` | Which evidence kinds go into the archive |
| `retain_days` | `30` | Archives older than this are deleted |
| `max_bytes_per_repo` | `10 MiB` | When a repository's archives exceed this, the oldest are deleted first |

Evidence kinds accepted in `include`: `spec`, `final_review_summary`, `reviews`, `transcripts`,
`checkpoint`, `review_slices`. The implement path does not write plans or verify logs to disk. A
single run whose selected evidence exceeds `max_bytes_per_repo` keeps the spec and the final review summary,
drops the rest, and records what it dropped.

The task spec may also live in the source repository when the project wants it there. Nothing else from a run is
committed to source.

### 3. A clone is deleted only when nothing would be lost

`merge` and a future `clean` command delete a clone only when, under the run's operation lock:

1. the clone's exact `HEAD` is on the configured remote (checked with `git ls-remote`, not with a local tracking
   ref);
2. the worktree is clean, including untracked files that are not ignored;
3. the evidence archive for the run has been written;
4. there is no stash, no assume-unchanged or skip-worktree entry, no in-progress merge, rebase or cherry-pick, and
   no registered linked worktree.

If check 4 fails, the clone is kept and reported, not deleted. These states are not backed up (see Context), so
the bridge will not destroy them silently. The user can resolve them or delete the clone deliberately.

Ignored files, such as `target/`, are treated as rebuildable and are not preserved.

### How this meets ADR-0041 §2

| Finding | Resolution |
|---|---|
| WRONG-1: merge deletes original history and evidence | The original commits are pushed before merge, and the evidence archive is written before deletion (§3 checks 1 and 3) |
| WRONG-2: hidden state destroyed by reap | Stashes and index flags block deletion (§3 check 4); untracked files are committed or block deletion (check 2) |
| WRONG-3: deleting a parent breaks linked worktrees | A registered linked worktree blocks deletion (§3 check 4) |
| SMELL: observe-lock-then-delete race | Deletion happens while holding the run's operation lock, not after observing it free |

## Consequences

- **Laptop loss loses only unpushed work since the last stable point**, plus local evidence. That is the same
  exposure any developer has, and a whole-disk backup covers the rest.
- **No custody stage needs off-laptop infrastructure beyond the Git remote** the project already uses.
- **The ADR-0041 code is parked.** The capsule schemas, export, seal and V/G/P restore crates stay merged and
  unwired. Removing them is a separate cleanup decision and is not required by this ADR.
- **ADR-0041's 2B3d, parent 2B gate and stages 3–5 stop.** The 2B3d design synthesis is kept as a record.

## Implementation

One slice:

1. `[custody]` and `[evidence]` config sections, validated by `a2a-bridge validate`.
2. Commit-and-push at the stable points in `implement`, `--resume`, and `merge`.
3. The evidence archive writer and its retention sweep (age, then size).
4. The §3 deletion gate, replacing the unconditional reap in `merge`.

Tests:

- a merge with an unpushed `HEAD` keeps the clone;
- a stash, an assume-unchanged entry, an in-progress merge, and a linked worktree each keep the clone;
- an untracked file is committed by the WIP commit;
- retention deletes by age and then by size, oldest first, and a single oversized run keeps the spec and summary;
- a push failure keeps the clone and reports the failure.
