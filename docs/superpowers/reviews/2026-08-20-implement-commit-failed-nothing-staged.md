# Bridge defect — `implement` attempted a commit with nothing staged, then lost the run

**Observed:** A2a-2 dispatch, clone `impl-53822-cjq6f6ul`, base `3963e056`.
The run ended with `Error: "git commit failed: "` — note the empty stderr — and
no commit, no branch handoff, and no clone-left message.

## What actually happened

`[MEASURED]` The agent's work was complete and correct. Its working tree held
380 insertions in `crates/bridge-worktree/src/sweep/checked_scan.rs` containing
all ten required scenarios. Run on the host, that tree gives
`cargo test -p bridge-worktree --locked` **exit 0, 302 passed** against main's
292 — exactly the +10 expected, zero failures.

Nothing was staged: `git diff --cached --name-only` returns nothing, and
`git status --porcelain` shows ` M` (index column blank).

## The empty stderr is diagnostic

Reproduced in the clone:

```
$ git -c user.name=t -c user.email=t@t commit --no-verify -m probe
EXIT=1
stdout: "no changes added to commit (use \"git add\" and/or \"git commit -a\")"
stderr: (empty)
```

`git commit` writes "nothing to commit" to **stdout**, not stderr.
`host_commit_argv_run` (`crates/bridge-controller/src/implement.rs:413`) formats
its error as `format!("git commit failed: {}", err.trim())` using **stderr
only**, so the operator-visible message is truncated to
`git commit failed: ` with the actual cause discarded.

## The real defect

`implement.rs` already models this case. `stage_state` classifies
`git status --porcelain` into `Staged` / `DirtyUnstaged` / `Clean`, and `decide`
maps `DirtyUnstaged` to `Action::NoCommitDirty`, whose handler prints *"agent
edited but staged NOTHING — NOT committing (agent owns staging). Clone left at
… for inspection."* and returns `Ok(())`.

That path did not run. `decide` must have received `Staged`, since only
`Action::Commit` reaches `host_commit`. So between the stage check
(`main.rs` ~3318) and the commit (~3363) the index went from non-empty to empty.
Nothing in that span mutates git — it is `commit_message`, `decide`, then
`host_commit` — so the unstaging happened outside that code path.

**Not yet proven:** what unstaged it. Candidates worth separating are the warm
container writing to the shared clone after the workflow returned, and a `git`
invocation inside the agent's own session landing late. I have not run a probe
that discriminates them and am not claiming one.

## Impact

Two distinct failures, one of them silent data loss:

1. **The operator cannot see why.** The error message drops the only text git
   produced. A run that should have printed the friendly `NoCommitDirty`
   guidance instead printed nine characters and a colon.
2. **The clone path is not printed.** `Action::Commit`'s error path returns
   `Err(e)` without the *"clone left at …"* line that `Abort`, `NoCommitClean`,
   and `NoCommitDirty` all print. A complete, correct, uncommitted 380-line
   work product was one `containers reap` away from being lost, and the operator
   was given no pointer to it.

## Suggested fixes

- In `host_commit_argv_run`, include stdout in the error when stderr is empty.
  Exit status is not behavioral evidence; read what the command actually
  produced.
- On the `Action::Commit` error path in `main.rs`, print the clone path exactly
  as the other three actions do, so a failed commit never strands work silently.
- Consider re-checking `stage_state` immediately before `host_commit` and
  falling back to the `NoCommitDirty` message rather than attempting a commit
  that cannot succeed.

## Recovery taken

Preserved `git diff` to a patch, applied it to `a2a/a2a2-recovered` off
`origin/main`, committed with the implementor attributed as author, and pushed.
Verified byte-identical to the agent's working tree. Host suite on the recovered
branch: 302 passed, 0 failed.

The A2a-2 handoff was never written — the agent had not reached that step — so
it remains outstanding.
