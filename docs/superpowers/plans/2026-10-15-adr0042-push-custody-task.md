---
task-type: implement
---
# ADR-0042 implementation: push-based custody, evidence retention, safe clone deletion

## Description

Implement [ADR-0042](../../adr/0042-push-based-custody-and-evidence-retention.md) (accepted 2026-10-15) for the
`a2a-bridge implement` / `--resume` / `merge` path. Today `merge` deletes the implement clone unconditionally after
a successful landing (`crates/bridge-controller/src/merge.rs:863`). That destroys the clone's original commit,
which `merge` re-authors, and its run evidence, and it can destroy a stash, index-flag edits or a linked worktree
(ADR-0041 §2 WRONG-1/2/3). After this work:

- the run's commit is pushed from the host to a configured remote at each stable point;
- a small, configurable evidence archive is written per run and pruned by age and size;
- `merge` deletes a clone only when nothing in it would be lost.

The work is three serial increments. **Each increment is its own `implement` run and PR, with a review cap of two
rounds, declared now.** At the cap, apply the convergence rule (fold if converging, park and escalate if
open-class). I2 builds on I1's merged code and I3 on I2's.

### Facts this spec relies on (verified at `main` `44950581`)

- **Clone origin.** `git clone --no-hardlinks <--repo path>` (`ctl/implement.rs:19–26`): the clone's `origin` is
  the **local source repo path**, never a hosted URL. `merge` pushes to `ck.source_repo` by path
  (`ctl/merge.rs:448–459`). **No code pushes to a network remote today.**
- **Identifiers.** The run id is `task_id = impl-<pid>-<nonce8>` (`ctl/implement.rs:249`), the same value as
  `resume_id`. The branch is `implement/<task_id>`.
- **Commits.** There is one commit per run:
  - first `host_commit` (`bin/a2a-bridge/src/main.rs:3765`);
  - then `host_amend_commit` after each fix (`ctl/tweak.rs:270`);
  - `restore_branch` (`ctl/tweak.rs:284`) moves the branch back to an earlier known SHA;
  - `NoCommitDirty` (`main.rs:3734–3761`) leaves uncommitted work and makes no commit.
- **Checkpoint.** `CLONE/.git/a2a-bridge/implement-checkpoint.json`, written atomically (`ctl/implement_resume.rs:91–107`).
- **Evidence today:**
  - the task spec is `CLONE/.git/A2A_TASK.md`;
  - the impl agent's transcripts are under `CLONE/.git/a2a-bridge/codex-sessions/`;
  - review slices are under `.git/a2a-bridge/review-slices/`;
  - **review verdict text is not persisted**: `run_review_step` returns `(outcome, synth)` (`main.rs:2513`, stderr at
    2641–2646);
  - verify logs and plans are not written to disk on the implement path.
- **Merge locking.** `merge_clone` holds the run's operation lock through push and reap (`ctl/merge.rs:634, 673`).
  A fresh `implement` loop does not hold it.
- **Existing gates to reuse.** `storage reap --clones` (`bin/a2a-bridge/src/storage_reap_clones.rs`) already gates
  deletion on:
  - `ls-files -v` index flags (`:1149`, pure parser `:1669`);
  - a ref sweep including `refs/stash` (`:1476–1501`);
  - linked worktrees and clone shape.
- **Tools available.**
  - Platform state dir: `select_platform_history` (`crates/bridge-store/src/sqlite.rs:15499`). Order is
    `A2A_BRIDGE_STATE_DIR`, then macOS `~/Library/Application Support/a2a-bridge`, then the XDG state dir.
  - `tar` and `flate2` are already dependencies of `bin/a2a-bridge`.
- **CI triggers** only on pushes to `main` and `feat/**` (`.github/workflows/ci.yml`), so `wip/**` pushes start no CI.

### Increment I1: config and push at stable points

1. **`[custody]` config section** in `bin/a2a-bridge/src/config.rs` (`deny_unknown_fields`, every key optional):
   - `push` (bool, default `true`);
   - `remote` (string, default `"origin"`);
   - `ref_prefix` (string, default `"wip/"`);
   - `delete_remote_on_merge` (bool, default `false`; consumed in I3);
   - `push_timeout_secs` (u64, default `120`, must be ≥ 1).

   `a2a-bridge validate` must reject invalid values: an empty `remote`, or a `ref_prefix` that fails
   `git check-ref-format` once combined with a sample run id. Today `validate` never calls `to_config()` for
   `[implement]`/`[merge]`; `[custody]` must be value-validated in `validate_config_contents`.

2. **Remote resolution.**
   - `remote` names a remote **of the source repo**, not of the clone. Resolve it once per run with
     `git -C <source_repo> remote get-url --push <remote>`.
   - If the source has no such remote, push is **disabled for this run**. Print one warning and record the reason
     in the checkpoint; the run itself does not fail.
   - The resolved URL is recorded in the checkpoint.
   - `--resume` re-resolves the URL and prints a warning if it changed.

3. **Push points.** Push the clone's current `HEAD` to `refs/heads/<ref_prefix><task_id>`:
   - after the first `host_commit`;
   - after every `host_amend_commit`;
   - after `restore_branch`;
   - before `merge` lands, only when the recorded last-pushed SHA differs from HEAD.

4. **Push mechanics.**
   - All push mechanics are host-side and use the host's git credentials. Agents and containers never receive any.
   - Command: `git -C <clone> -c core.hooksPath=/dev/null push --no-verify --porcelain <url> <sha>:refs/heads/<ref>`
     with `--force-with-lease=refs/heads/<ref>:<expected>`.
     - `<expected>` is the checkpoint's last-pushed SHA, or empty for the first push, meaning the ref must not exist.
     - This lets the run update its own branch after an amend but never overwrite someone else's branch.
   - Environment: `GIT_TERMINAL_PROMPT=0`, plus `GIT_SSH_COMMAND` with `-o BatchMode=yes` unless the user already
     set it. The push is killed after `push_timeout_secs`.

5. **Push failure is not fatal.**
   - It prints a warning and records `{ref, url, last_pushed_sha, last_error}` in a new optional checkpoint field
     `custody`. The field uses `#[serde(default)]` so older checkpoints still load.
   - A stale-lease refusal (the ref exists with an unexpected SHA) disables push for the rest of the run and says so.

6. **Uncommitted work.** On the `NoCommitDirty` arm, and at any terminal exit where `git status --porcelain` is
   non-empty, write a WIP snapshot without touching the worktree, the index or the branch:
   - Write a temporary index with `GIT_INDEX_FILE` and run `git add -A` into it. Ignored files are excluded.
   - `write-tree`, then `commit-tree` with parent `HEAD` (or none if HEAD is unborn) and message `wip snapshot <task_id>`.
   - Push it to `refs/heads/<ref_prefix><task_id>-dirty` with the same lease rules.
   - Record the SHA in the checkpoint.
   - Do not move `implement/<task_id>`. The existing dirty-refusal in `reconcile_head` is unchanged.

**I1 tests** use a bare repository on the local filesystem as the "remote", registered as `origin` in a fixture
source repo:
- the first commit is pushed, and the ref equals HEAD;
- an amend force-updates the run's own ref;
- a pre-existing foreign ref at the target name is not overwritten: lease refusal, push disabled, run continues;
- a missing source remote disables push with one warning, and the run continues;
- an unreachable URL (nonexistent path) fails within the timeout and does not fail the run;
- the `NoCommitDirty` snapshot captures an untracked non-ignored file and excludes an ignored file, while the
  worktree, index and branch are byte-for-byte unchanged;
- an old checkpoint without `custody` still loads;
- `validate` rejects `remote = ""` and a bad `ref_prefix`;
- no push happens when `push = false`.

### Increment I2: evidence archive and retention

1. **Persist review text.** After each review, write the review synthesis to
   `CLONE/.git/a2a-bridge/reviews/review-<attempt>.md`, capped at 256 KiB and truncated with a marker. Record the
   final attempt number in the checkpoint.

2. **`[evidence]` config section:**

   | Key | Default | Constraint |
   |---|---|---|
   | `store` | `"local"` | the only accepted value today |
   | `path` | `<platform state dir>/evidence` | absolute, if set |
   | `include` | `["spec", "final_review_summary"]` | each kind from the closed vocabulary below |
   | `retain_days` | `30` | ≥ 1 |
   | `max_bytes_per_repo` | `10485760` | ≥ 65536 |

   - Extract the platform-dir logic from `select_platform_history` into a small shared helper rather than
     duplicating it.
   - Closed vocabulary for `include`. `validate` rejects anything else.

     | Kind | Source |
     |---|---|
     | `spec` | `A2A_TASK.md` |
     | `final_review_summary` | the last `reviews/review-<n>.md` |
     | `reviews` | all `reviews/*.md` |
     | `transcripts` | `codex-sessions/**` |
     | `checkpoint` | `implement-checkpoint.json` |
     | `review_slices` | `review-slices/*.md` |

   - **Amend ADR-0042 §2 in the same PR.** The ADR lists `plans` and `verify_logs`, but the implement path writes
     neither to disk, so replace its list with this one.

3. **Archive.** Write one `tar.gz` per run to
   `<path>/<repo-key>/<task_id>.tar.gz`:
   - `<repo-key>` is `<sanitized source basename>-<first 12 hex of SHA-256 of the canonical source path>`.
   - The archive contains the included files plus a `manifest.json`:
     - `schema: "a2a-evidence.v1"`, `task_id`, `source_repo`, `branch`, `final_commit`, `terminal_phase`, `created_at`
       (RFC 3339 UTC);
     - `included: [{kind, path, bytes, sha256}]`, `dropped: [{kind, reason}]`.
   - Write it to `<task_id>.tar.gz.tmp` in the same directory, then rename it into place.
   - Write it when the run reaches a terminal phase (`write_terminal`, `main.rs:3285`), and rewrite it after a
     successful landing, so `terminal_phase` reads `merged`.
   - A missing optional source is dropped with reason `absent`. Archive failure prints a warning and never fails the run.

4. **Oversize.** If one run's selected content exceeds `max_bytes_per_repo`:
   - keep `spec` and `final_review_summary`;
   - drop the other kinds in reverse vocabulary order until the archive fits, recording each as `dropped: over_cap`;
   - if those two alone exceed the cap, write them anyway and warn.

5. **Retention sweep.** Run it after each archive write, for that `<repo-key>` directory only:
   1. Delete archives whose manifest `created_at` is older than `retain_days`.
   2. Then delete oldest first (by `created_at`, ties by file name) until the directory total is ≤ `max_bytes_per_repo`.
   3. Never delete the archive just written.

   Only regular files named `impl-*.tar.gz` are considered. Symlinks are never followed, and unreadable or
   unparseable archives are left in place and reported. A `*.tmp` older than 1 hour is removed.

**I2 tests:**
- default `include` produces exactly the spec and summary, with a correct manifest and digests;
- each vocabulary kind is included when configured;
- an unknown kind is rejected by `validate`;
- oversize drops in the specified order, and spec and summary are kept even when over the cap;
- the age sweep deletes an old archive and keeps a fresh one;
- the size sweep deletes oldest first, never the current archive;
- a symlink and a foreign file in the directory are untouched;
- the rewrite at merge sets `terminal_phase = merged`;
- an archive failure (read-only directory) does not fail the run;
- review text is truncated at 256 KiB with the marker.

### Increment I3: safe deletion in `merge`

Replace the unconditional `reap_clone` call at `ctl/merge.rs:863` with a gate evaluated **while the operation lock
is still held**. Delete only if every check passes; otherwise keep the clone and print each failed check with the
clone path. **The exit code is unchanged.** A kept clone is not a merge failure.

1. **On remote.** `[custody].push` is true, and `git ls-remote <url> refs/heads/<ref_prefix><task_id>` returns
   exactly the clone's HEAD SHA. Use the URL resolved for this run. Local tracking refs never satisfy this check.
   When `push = false`, this check fails with reason "push disabled", so clones are kept.
2. **Clean.** `git status --porcelain` is empty, which includes untracked non-ignored files. Ignored files are
   allowed.
3. **Evidence.** The run's archive exists and its manifest `task_id` matches.
4. **Nothing unbacked.** All of these must hold:
   - no `refs/stash`;
   - no assume-unchanged, skip-worktree or sparse entries;
   - no in-progress operation: no `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `rebase-merge/`, `rebase-apply/`
     or `sequencer/` under the clone's git dir;
   - no registered linked worktree: `git worktree list --porcelain` has exactly one entry.

   **Reuse the existing parsers** from `storage_reap_clones.rs` for the stash and index-flag checks. Move them into
   a shared module if needed rather than copying them.

After a successful delete, if `delete_remote_on_merge = true`, delete the remote run branch with a leased push
(`--force-with-lease=refs/heads/<ref>:<HEAD>`). A failure there is a warning only.

**I3 tests** cover:
- each check failing alone keeps the clone (8 cases):
  - remote ref absent;
  - remote ref at a different SHA;
  - push disabled;
  - dirty;
  - archive missing;
  - stash;
  - skip-worktree;
  - linked worktree;
- an in-progress `MERGE_HEAD` keeps the clone;
- all checks passing deletes the clone;
- `delete_remote_on_merge` removes the remote ref only after a delete;
- the already-integrated success path uses the same gate;
- exit codes are unchanged in every case.

**Before the change, the same tests show the old behavior:** the clone is deleted despite a stash, a skip-worktree
entry, or a linked worktree. State this result in the PR.

### Out of scope

- `storage reap --clones` stays on its existing gates; adopting the ADR-0042 check 1 there is a follow-up.
- Remote evidence stores.
- Reflog-only and unreachable objects, stashes, in-progress state, and per-clone config and hooks (ADR-0042
  Context).
- `run-workflow` artifacts.
- Removing the parked ADR-0041 code.

## Acceptance Criteria

- I1, I2 and I3 each merged as separate PRs, each with its tests above passing.
- **Local verify per owner rule:** `cargo fmt --check`, `cargo clippy` with `-D warnings`, and tests for the touched
  crates only, with `CARGO_INCREMENTAL=0`. The full workspace suite runs in CI. Each PR states the local scope and
  the CI result.
- No agent or container receives git credentials. No push goes anywhere except `[custody].remote`'s resolved URL
  and the existing local-path landing push.
- ADR-0042 §2's evidence-kind list matches the implemented vocabulary (I2).
- `AGENTS.md` §3 and `docs/onboarding.md` describe `[custody]`, `[evidence]` and the kept-clone messages (I3).

## Files

- I1: `bin/a2a-bridge/src/config.rs`, `bin/a2a-bridge/src/main.rs` (implement/resume/merge wiring),
  `crates/bridge-controller/src/implement.rs`, `ctl/implement_resume.rs` (checkpoint field), new
  `crates/bridge-controller/src/custody_push.rs`.
- I2: `bin/a2a-bridge/src/main.rs` (review persistence, terminal hook), new `bin/a2a-bridge/src/evidence.rs`,
  `config.rs`, the shared platform-dir helper, `docs/adr/0042-…md`.
- I3: `crates/bridge-controller/src/merge.rs`, the shared stash and index-flag parsers out of
  `bin/a2a-bridge/src/storage_reap_clones.rs`, `AGENTS.md`, `docs/onboarding.md`.

## Spec Refs

- ADR-0042 (governing)
- ADR-0041 §2 (findings to close)
- ADR-0027 (merge)
- ADR-0026 (checkpoint and resume)
- ADR-0025 (operation lock)

## Commit Message

Per increment, for example: `feat(custody): push run branch at stable points (ADR-0042 I1)`.
