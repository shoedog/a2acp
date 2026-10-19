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
| `review_history` | `true` | Secure each reviewed revision with an immutable history ref, independently of `push` |
| `remote` | `origin` | Remote to push to; may name a private mirror |
| `ref_prefix` | `wip/` | Branch prefix; the run branch is `<prefix><run-id>` |
| `delete_remote_on_merge` | `true` | Delete the run branch after a successful merge |

The pushed branch keeps the original commits. That is what makes `merge`'s re-authored commits safe: the originals
remain on the remote.

On a public repository, the run branch is public. Set `remote` to a private mirror if that is not acceptable.

Each actual implementation review, including resume, and local `run-workflow` review over a clone
reserves a distinct invocation and publishes `refs/review-history/<run-id>/<invocation>` before dispatch.
These refs use the source repository's configured remote and the isolated, bounded host transport.
An existing equal ref permits recovery; a different SHA is a collision and is never overwritten.
Dirty checkouts and hidden index flags are refused. Reviewers run in a separate read-only checkout of
the preserved SHA inside the clone's mount, so an editor changing and restoring the original checkout
cannot change the reviewed bytes. Tracked symlinks are preserved when their resolved targets stay
inside the snapshot; escaping or unresolvable links are explicitly refused.
Workflow preservation uses all frozen admitted checkout identities, including configured agent cwd;
multi-checkout, subdirectory, and worktree-managed reviews are refused until snapshot binding supports them.
Review designation is explicit: every `implement` review step (the configured `[review].workflow`
and its `-light`/`-thorough` variants) is a review. A local `run-workflow` is a review only when its
`[[workflows]]` entry sets `review = true` (default `false`), independently of its id, prompt or brief.
The shipped review workflows set that flag, including the OpenCode variants.
Served workflows are outside this preservation scope: the client submits custom and review workflows
without classification or refusal, and neither client nor server preserves their revisions or labels.
Server-owned preservation is a documented limitation and remains future work.
Output-side diagnostics are deferred until after review publication, so an in-clone `--out` does not
dirty an initially clean input. Missing custody origin is a preservation error, never a review bypass.
History refs on a public remote are public; select a private mirror with `remote` when needed.
Explicit `review_history = false` disables new history publication and linkage without changing ordinary custody.

History refs have indefinite retention. Merge, every reap disposition, and archive retention never delete
them. Pruning requires a separate explicit command or policy, outside this implementation.

### 2. Evidence is kept by a configurable policy

Before and after every review, at merge, and at the terminal state of an unmerged run, the bridge
refreshes the external evidence archive. Mandatory manifest records retain every invocation's workflow,
reviewed SHA, exact ref, redacted remote, status, verdict and findings summary, independently of `include`
and optional oversized-payload dropping. Interrupted/failed reviews remain incomplete/inconclusive.
Publication failure refuses review or stops the repair loop; completed labels stay in the clone, and
archive verification refuses automatic deletion until all local history records are published.
Later reviews and merged-phase rewrites preserve earlier records; archive age/size retention is unchanged.

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

`merge` and confirmed merged `storage reap --clones` delete a clone only when, under the run's operation lock:

1. the clone's exact `HEAD` is on the configured remote (checked with `git ls-remote`, not with a local tracking
   ref);
2. the worktree is clean, including untracked files that are not ignored;
3. the evidence archive for the run has been written;
4. there is no stash, no assume-unchanged or skip-worktree entry, no in-progress merge, rebase or cherry-pick, and
   no registered linked worktree.

If check 4 fails, the clone is kept and reported, not deleted. These states are not backed up (see Context), so
the bridge will not destroy them silently. The user can resolve them or delete the clone deliberately.

Ignored files, such as `target/`, are treated as rebuildable and are not preserved.

### 4. Landing provenance and parked-clone decisions

New landing commits contain exactly one canonical `A2A-Run`/`A2A-Original` trailer pair. Supplied
conflicts are removed. Already-integrated no-ops add no empty commit. Automatic merged reap requires the
exact pair reachable from source `main`; another branch, wrong run or original SHA does not qualify.
Stable patch IDs identify high-confidence patch equivalence; all changed paths matching together in a
single main commit within 14 days after the clone's last commit identify low-confidence content
equivalence. These are adjudication evidence, never proven run provenance. Probe failures are unknown.

Reap reports run, path, source, confidence, matched commit/reason and deletion blockers. It reads the run's
recorded configuration; unavailable/invalid metadata or configuration retains the clone. The shared gate
checks configuration, remote HEAD, cleanliness, configured archive, refs and repository shape, including
index flags, nested repositories and linked worktrees. It accepts ordinary ignored build output. Receipts
remain, but no duplicate evidence copy is required. The run's recorded runtimes must affirm the absence
of containers for its clone and run generations; unavailable runtimes retain the clone, including under
explicit remove. Chained-origin provenance uses the terminal root, separately from immediate-source custody.

Owner decision files use `run_ids` for explicit remove and optional `keep_run_ids` for keep, with
`authorized_by` and `reason`. Contradictions refuse. Keep or no decision preserves parked clones and refs.
Explicit remove is an owner-authorized discard, waiving merged classification and Git-state custody checks
(on-remote, clean, archive, refs and shape allowlists). It retains every live-use, operation-lock,
protected-root, path-identity and container safeguard. Its archive retains the actual terminal phase.
Source resolution is best-effort for authorized discard: a missing source permits local removal and leaves
a receipt reporting remote cleanup unavailable; existing sources retain their containment and identity checks.

After successful local removal, recorded normal and dirty remote refs are deleted with existing leases.
Gate failure or local-removal failure leaves remote refs untouched. A changed foreign ref is retained and
reported. Remote rejection reports incomplete cleanup and remaining refs; successful landing stays exit 0.
`delete_remote_on_merge = true` is the default; operators may explicitly opt out for merged cleanup.

### 5. Bounded archive memory

All selected sources are opened and validated before any publication side effect, and their handles are
retained for streaming. Payloads and compressed archives stream through fixed-size buffers to a staging
file created with mode `0600` on Unix, regardless of umask. Only file names, sizes, SHA-256 digests and the manifest scale with file count. Verification streams tar entries and
drains gzip EOF to check compressed checksums. Compressed-size dropping, protected evidence, retention
and atomic publication behavior remain intact. At fixed file counts, the declared regression bound is
less than 16 MiB additional peak RSS for 16 MiB versus 256 MiB payloads, including a single large file.
Linux regression tests measure fresh child processes at one and four files.
The Linux arm64 / Rust 1.94 control measured peak RSS at 6,956 / 6,948 KiB for one payload file
(16 / 256 MiB) and 6,976 / 6,972 KiB for four payload files, plus a fixed small spec. The isolated
unchanged-base control measured 72,188 / 793,152 KiB for one file and failed the same growth bound.
The test covers both archive creation and complete digest/checksum verification in each child.


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
