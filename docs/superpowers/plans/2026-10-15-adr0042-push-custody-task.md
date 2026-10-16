---
task-type: implement
---
# ADR-0042 implementation: push-based custody, evidence retention, safe clone deletion

Revision 2 (2026-10-15) folds spec-review round 1. The round-1 ledger is at the end.

## Description

Implement [ADR-0042](../../adr/0042-push-based-custody-and-evidence-retention.md) (accepted 2026-10-15) for the
`a2a-bridge implement` / `--resume` / `merge` path. Today `merge` deletes the implement clone unconditionally after
a successful landing (`crates/bridge-controller/src/merge.rs:863`). That destroys the clone's original commit
(which `merge` re-authors) and its run evidence, and it can destroy a stash, index-flag edits, side refs, submodule
history or a linked worktree (ADR-0041 §2 WRONG-1/2/3). After this work:

- the run's commit is pushed from the host to a configured remote before each review and before merge;
- a small, configurable evidence archive is written per run and pruned by age and size;
- `merge` deletes a clone only when nothing in it would be lost.

The work is four serial increments: **I1a, I1b, I2, I3**. Each is its own `implement` run and PR, with a review
cap of **two rounds, declared now**. At the cap, apply the convergence rule (fold if converging, park and escalate
if open-class). Each increment builds on the previous one's merged code.

### Facts this spec relies on (verified at `main` `44950581`)

- **Clone origin.** `git clone --no-hardlinks <--repo path>` (`ctl/implement.rs:19–26`): the clone's `origin` is
  the **local source repo path**, never a hosted URL. `merge` pushes to `ck.source_repo` by path
  (`ctl/merge.rs:448–459`). **No code pushes to a network remote today.**
- **Identifiers.** The run id is `task_id = impl-<pid>-<nonce8>` (`ctl/implement.rs:249`), the same value as
  `resume_id`. The branch is `implement/<task_id>`.
- **Commits.** There is one commit per run:
  - first `host_commit` (`bin/a2a-bridge/src/main.rs:3765`), which commits only the **agent-staged** index;
  - then `host_amend_commit` after each fix (`ctl/tweak.rs:270`);
  - `restore_branch` (`ctl/tweak.rs:284`) moves the branch back to a trusted SHA;
  - `reset_worktree_to_head` (`ctl/implement.rs:547`) discards unstaged and untracked leftovers before every
    verify, **by design**, so verify tests exactly the committed tree.
- **Terminal arms.** The `Abort`, `NoCommitClean` and `NoCommitDirty` arms and an initial commit failure
  (`main.rs:3734–3780`) write no checkpoint and bypass `write_terminal` (`main.rs:3285`).
- **Checkpoint.** `CLONE/.git/a2a-bridge/implement-checkpoint.json`, written atomically
  (`ctl/implement_resume.rs:91–107`). It sits inside the clone, so **the implement agent can read it**.
- **Evidence today:**
  - the task spec is `CLONE/.git/A2A_TASK.md`;
  - transcripts are under `CLONE/.git/a2a-bridge/codex-sessions/`;
  - review slices are under `.git/a2a-bridge/review-slices/`;
  - **review verdict text is not persisted**: `run_review_step` returns `(outcome, synth)`, `main.rs:2513`.
- **Merge.** `merge_clone` holds the run's operation lock through landing and reap (`ctl/merge.rs:634, 673`). The
  shared success arm covers both normal and already-integrated landings. Dirty clones are refused at preflight
  with exit 1 (`ctl/merge.rs:747–749`).
- **Existing gates to reuse.** These live in `bin/a2a-bridge/src/storage_reap_clones.rs`:
  - the `ls-files -v` index-flag parser (`:1149`, `:1669`);
  - the ref sweep over `refs/heads/*`, `refs/tags/*` and `refs/stash` (`refs_disposition`, `:1476–1501`);
  - the submodule checks (`:1726–1779`).
- **Tools available.**
  - Platform state dir: `select_platform_history` (`crates/bridge-store/src/sqlite.rs:15499`).
  - `tar` and `flate2` are already dependencies of `bin/a2a-bridge`.
- **CI.** The push workflow runs only for `main` and `feat/**`, so pushing `wip/**` starts no CI. Updates to an open
  PR targeting `main` still trigger CI.

### Common rules for every Git transport call (push and ls-remote)

These apply in I1a and I3.

- **Host side only.** The host's git credentials are used. Agents and containers never receive any.
- **The destination is resolved from the source repo, not the clone.**
  1. Resolve it with `git -C <source_repo> remote get-url --push <remote>`.
  2. A relative filesystem URL is resolved against the source repo's working directory, giving an absolute path.
  3. The result is the **transport URL**. It is held in memory only and re-resolved whenever needed. It is never
     written to the checkpoint, the archive or stderr.
  4. What gets persisted and printed is the **destination identity**: the URL with any userinfo (user, password,
     token) removed. Git diagnostics are redacted the same way before they are printed or stored.
- **No redirection.**
  - If the clone's effective config (`git -C <clone> config --get-regexp '^url\..*\.(insteadof|pushinsteadof)$'`,
    case-insensitive) has any entry, the transport call is **refused**, because the agent can write the clone's
    config.
  - Every call passes these options:
    - `-c push.recurseSubmodules=no`
    - `--no-follow-tags`
    - `-c core.hooksPath=/dev/null`
    - `--no-verify`
- **Bounded.**
  - `GIT_TERMINAL_PROMPT=0`.
  - `GIT_SSH_COMMAND` gets `-o BatchMode=yes` unless the user already set it.
  - A deadline of `push_timeout_secs` applies to push and ls-remote alike. On expiry the child process group is
    killed and reaped, and the call reports a timeout.

### Increment I1a: config, push before review and merge, resume

1. **`[custody]` config section** (`deny_unknown_fields`, every key optional):
   - `push` (bool, default `true`);
   - `remote` (string, default `"origin"`);
   - `ref_prefix` (string, default `"wip/"`);
   - `delete_remote_on_merge` (bool, default `false`; consumed in I3);
   - `push_timeout_secs` (u64, default `120`, must be ≥ 1).

   `a2a-bridge validate` checks values in `validate_config_contents`: it rejects an empty `remote`, and a
   `ref_prefix` that fails `git check-ref-format` once combined with a sample run id.

2. **Push points.** Push the clone's HEAD to `refs/heads/<ref_prefix><task_id>`:
   - **before every review**, including the first review after `--resume`;
   - **before merge lands**.

   In both cases the push is skipped when the checkpoint already records a successful push of the same
   `(destination identity, ref, HEAD)`. Every new commit or amend is reviewed next, so "before every review"
   covers each one and also covers a crash between commit and push.

3. **Lease and ownership.**
   - Use `--force-with-lease=refs/heads/<ref>:<expected>` and `--porcelain`.
   - `<expected>` is the SHA last pushed **to the same destination identity and ref**. A lease SHA is never reused
     across a changed destination or ref.
   - With no recorded successful push, `<expected>` is empty, meaning the ref must not exist. In that case only
     porcelain proof that a **new ref was created** (`*` flag) counts as success.
   - An "up to date" or existing ref is a **collision**, including when the foreign ref already equals HEAD. A
     collision or a stale-lease refusal disables push for this run and is recorded.

4. **Checkpoint.** Add an optional `custody` field with `#[serde(default)]`, so older checkpoints load through the
   real `--resume` and `merge` paths. It records:
   - `destination` (the identity, never the transport URL);
   - `normal: {ref, last_pushed_sha?}` and `dirty: {ref, last_pushed_sha?}`, as separate records;
   - `disabled: Option<reason>`;
   - `last_error` (redacted).

   On `--resume` and `merge`, the current `[custody]` config wins. If the destination identity or ref differs from
   the record, the old lease SHAs are discarded and the next push is treated as a first push.

5. **Push failure is not fatal.** Print a warning, record it, and continue the run.

**I1a tests.** The "remote" is a bare repository on the local filesystem, registered in a fixture source repo.
- the first review is preceded by a push, and the ref equals HEAD;
- after an amend, the run's own ref is force-updated;
- a pre-existing foreign ref is not overwritten, including **the equal-SHA case**; push is disabled and the run
  continues;
- **resume after a crash between commit and push** pushes before the next review;
- a **source-relative URL** (`../backup.git`) reaches the source-relative repository, not a clone-relative one;
- an `https://user:token@…` URL never appears in the checkpoint or on stderr;
- a clone-local `url.*.pushInsteadOf` makes the push refuse;
- with `push.recurseSubmodules=on-demand` and a tag present, no other ref or repository changes;
- a **stalled transport**, for example a `GIT_SSH_COMMAND` that sleeps, times out within the deadline;
- a missing source remote disables push with one warning;
- `push = false` pushes nothing;
- an old checkpoint resumes;
- `validate` rejects the bad values.

### Increment I1b: terminal finalizer and the dirty snapshot

1. **Shared terminal finalizer.** It covers every handled terminal arm:
   - Approved and LoopStopped (`write_terminal`);
   - `Abort`, `NoCommitClean` and `NoCommitDirty`;
   - an initial commit failure;
   - a refused `--resume`.

   Where no checkpoint exists, it writes a minimal **no-commit checkpoint** (task id, source repo, branch, base
   commit, terminal phase, no current commit). I2 hooks the archive writer into this finalizer.

   An uncatchable process death (SIGKILL, power loss) is out of scope. The next `--resume` or `merge` pushes, and
   the I3 gate keeps the clone.

2. **Dirty snapshot.** In the finalizer, when the custody status check (below) finds non-ignored changes, write a
   snapshot without touching the worktree, the real index or any branch:
   1. Copy the real index to a private temporary file, so it stays a valid index and tracked-but-ignored files stay
      tracked.
   2. With `GIT_INDEX_FILE` pointing at that copy, run `git add -A`.
   3. `write-tree`.
   4. `commit-tree` with parent HEAD (none if unborn), the pinned bot identity and pins from `pin_prefix_argv`
      (`commit.gpgsign=false`, hooks off), and message `wip snapshot <task_id>`.
   5. Push it to `refs/heads/<ref_prefix><task_id>-dirty` under the lease rules, recorded in `custody.dirty`.

   The temporary index is removed on success and on failure.

3. **Custody status check.** Used here and in I3:
   `git -c core.untrackedCache=false status --porcelain=v1 --untracked-files=all --ignore-submodules=none`, run with
   `GIT_OPTIONAL_LOCKS=0`. A plain `status --porcelain` can hide untracked files under
   `status.showUntrackedFiles=no`.

**Not changed (round-1 #8, deferred):** `reset_worktree_to_head` still discards agent work that was left unstaged
before verify. That is existing, intended behavior, so verify tests exactly the committed tree. ADR-0042 protects
commits and evidence, not agent scratch.

**I1b tests:**
- each terminal arm runs the finalizer;
- a no-commit checkpoint is written for `NoCommitDirty`;
- the snapshot captures an untracked non-ignored file and keeps a tracked-but-ignored file;
- it excludes an ignored untracked file;
- the worktree, the real index bytes and every branch are unchanged;
- `status.showUntrackedFiles=no` does not hide an untracked file from the custody status check;
- a snapshot push collision is recorded and is not fatal.

### Increment I2: evidence archive and retention

1. **Persist review text.** After each completed review, write the synthesis to
   `CLONE/.git/a2a-bridge/reviews/review-<attempt>-<invocation>.md`. `<invocation>` is a counter stored in the
   checkpoint, so a resume never overwrites earlier reviews. Each file is capped at 256 KiB and truncated with a
   marker. `final_review_summary` is the newest *completed* review. An interrupted review writes nothing.

2. **`[evidence]` config section:**

   | Key | Default | Constraint |
   |---|---|---|
   | `store` | `"local"` | the only accepted value |
   | `path` | `<platform state dir>/evidence` | absolute, if set |
   | `include` | `["spec", "final_review_summary"]` | closed vocabulary below |
   | `retain_days` | `30` | ≥ 1 |
   | `max_bytes_per_repo` | `10485760` | ≥ 65536 |

   - Extract the platform-dir logic from `select_platform_history` into a shared helper.
   - Closed vocabulary for `include`:

     | Kind | Source |
     |---|---|
     | `spec` | `A2A_TASK.md` |
     | `final_review_summary` | the newest completed review |
     | `reviews` | all `reviews/*.md` |
     | `transcripts` | `codex-sessions/**` |
     | `checkpoint` | the checkpoint |
     | `review_slices` | `review-slices/*.md` |

   - **Amend ADR-0042 §2** to this list. The ADR's `plans` and `verify_logs` are never written to disk on the
     implement path.

3. **Archive.**
   - One `tar.gz` per run at `<path>/<repo-key>/<task_id>.tar.gz`.
     - `<repo-key>` is `<sanitized source basename>-<first 12 hex of SHA-256 of the canonical source path>`.
   - **The canonical destination must not be inside the run's clone**, including through symlink aliases. If it is,
     no archive is written and a warning is printed; I3 then keeps the clone.
   - The archive holds a `manifest.json` with:
     - `schema: "a2a-evidence.v1"`, `task_id`, `source_repo`, `branch`;
     - `final_commit` (or null), `terminal_phase`, `created_at` (RFC 3339 UTC);
     - `included: [{kind, path, bytes, sha256}]`, `dropped: [{kind, reason}]`.
   - Publication: write `.<task_id>.tar.gz.a2a-tmp`, fsync it, then rename it into place.
   - **When it is written:**
     - from the I1b finalizer;
     - again after a successful landing, with `terminal_phase = merged`, through the post-landing hook (I3 §1).
   - Failure prints a warning and never fails the run.

4. **Accounting and oversize.**
   - **All sizes are stored `.tar.gz` byte lengths**, including the manifest.
   - When one run's archive exceeds `max_bytes_per_repo`, drop the selected kinds other than `spec` and
     `final_review_summary`, in reverse vocabulary order, until it fits. Record each as `dropped: over_cap`.
   - Oversize handling never adds a kind that `include` excluded.
   - If the protected kinds alone exceed the cap, write them anyway and warn.

5. **Retention sweep.** Run it after each archive write, for that `<repo-key>` directory only.
   - It runs while holding a per-directory lock: an exclusive `flock` on `<repo-key>/.lock`. The archive
     publication takes the same lock, so concurrent runs serialize.
   - **Owned files** are regular, non-symlink files whose names match `impl-*.tar.gz` and whose manifest parses with
     schema `a2a-evidence.v1`. Every other entry is left untouched.
   - **Stale temps:** only regular files matching `.impl-*.tar.gz.a2a-tmp` and older than 1 hour are removed.
   - Order:
     1. Delete owned archives whose `created_at` is older than `retain_days`.
     2. Delete owned archives oldest first (ties by name) until the owned total is ≤ `max_bytes_per_repo`.
   - The archive just written is never deleted. Excess that can't be removed is reported.

**I2 tests:**
- **content:** the default `include` produces exactly the spec and summary, with a correct manifest and digests;
- **kinds:** every vocabulary kind can be included; `validate` rejects an unknown kind;
- **review files:** reviews from a resumed run don't overwrite earlier ones, and an interrupted review is not chosen
  as the summary;
- **oversize:**
  - the drop order holds, measured in compressed bytes;
  - excluded kinds are never added back;
  - the protected kinds are kept even when over the cap;
- **sweep:**
  - age, then size, and the current archive is never deleted;
  - a foreign `notes.tmp`, a symlink, and an unparseable `impl-x.tar.gz` are left untouched;
  - two concurrent writers serialize;
- **destination and failure:**
  - a `path` inside the clone writes no archive;
  - the rewrite after merge sets `merged`;
  - an archive failure doesn't fail the run.

### Increment I3: post-landing hook and safe deletion in `merge`

1. **Post-landing interface.**
   - `merge_clone` takes a host-supplied hook, invoked after a successful landing (normal or already-integrated)
     and before the gate, **while the operation lock is still held**.
   - The binary's hook pushes HEAD if it isn't recorded as pushed (I1a) and rewrites the archive with `merged` (I2).
   - All merge entry paths pass it: standalone `merge`, fresh `implement --merge` and resumed `--merge`.

2. **Deletion gate.** It replaces the unconditional `reap_clone`. Delete only if every check passes; otherwise keep
   the clone and print each failed check with the clone path. **Exit codes are unchanged**: a clone kept after a
   successful landing exits 0, and preflight refusals stay exit 1.

   | # | Check | Passes when |
   |---|---|---|
   | 1 | On remote | `[custody].push` is true and `ls-remote` (common transport rules) shows `refs/heads/<ref_prefix><task_id>` at exactly HEAD. With `push = false` the reason is "push disabled". |
   | 2 | Clean | The custody status check (I1b §3) is empty. Ignored files are allowed. |
   | 3 | Evidence | The archive exists outside the clone, is fully readable, and its manifest has the expected schema, `task_id`, `source_repo`, `final_commit` = HEAD and `terminal_phase = merged`. Every listed file matches its recorded size and SHA-256. |
   | 4 | Refs covered | Every `refs/heads/*` and `refs/tags/*` tip is HEAD, an ancestor of HEAD, or contained in the source repo, using the existing `refs_disposition`. There is no `refs/stash`. |
   | 5 | Nothing unbacked | No assume-unchanged, skip-worktree or sparse entries (existing `ls-files -v` parser). No in-progress `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `rebase-merge/`, `rebase-apply/` or `sequencer/`. No initialized submodule or `.git/modules` (existing submodule checks). No linked worktree: `git worktree list --porcelain` has one entry. |

   Reuse the `storage_reap_clones.rs` helpers by moving them into a shared module, not by copying them.

3. **Remote cleanup.** After a successful delete, if `delete_remote_on_merge = true`, delete the run's remote refs
   with a leased push. A failure there is a warning only.

**I3 tests.** Each case keeps the clone and exits 0:
- remote ref absent;
- remote ref at a different SHA;
- push disabled;
- an `ls-remote` timeout with a stalled transport (the operation lock is released afterwards);
- dirtiness introduced **after preflight**, by a test seam;
- archive missing;
- archive digest mismatch;
- archive inside the clone;
- an unpushed side branch;
- a unique tag;
- a stash;
- a skip-worktree entry;
- `MERGE_HEAD`;
- an initialized submodule;
- a linked worktree.

Beyond the keep cases:
- all checks passing deletes the clone;
- the already-integrated path uses the same hook and gate;
- `delete_remote_on_merge` removes remote refs only after a delete;
- an initially dirty clone still exits 1 at preflight.

**Before the change, the same tests show the old behavior:** the clone is deleted with a stash, a skip-worktree
entry, a side branch or a linked worktree. State this result in the PR.

### Out of scope

- `storage reap --clones` stays on its existing gates; adopting check 1 there is a follow-up.
- Remote evidence stores.
- Reflog-only and unreachable objects, stash contents, in-progress state contents, and per-clone config and hooks
  (ADR-0042 Context). I3 keeps clones that have them; it does not preserve them.
- Agent work left unstaged before verify (round-1 #8).
- `run-workflow` artifacts.
- Removing the parked ADR-0041 code.

## Acceptance Criteria

- I1a, I1b, I2 and I3 each merged as separate PRs, each with its tests above passing.
- **Local verify per owner rule:** `cargo fmt --check`, `cargo clippy` with `-D warnings`, and tests for the touched
  crates only, with `CARGO_INCREMENTAL=0`. The full suite runs in CI. Each PR states the local scope and the CI
  result.
- No agent or container receives git credentials. No transport URL with credentials is persisted or printed.
- No push goes anywhere except the resolved `[custody].remote` destination and the existing local-path landing push.
- ADR-0042 §2's evidence-kind list matches the implemented vocabulary (I2).
- `AGENTS.md` §3 and `docs/onboarding.md` describe `[custody]`, `[evidence]` and the kept-clone messages (I3).

## Files

| Increment | Files |
|---|---|
| I1a | `bin/a2a-bridge/src/config.rs`, `bin/a2a-bridge/src/main.rs` (review and resume wiring), `ctl/implement_resume.rs` (checkpoint field), new `crates/bridge-controller/src/custody_push.rs` (transport rules, push, ls-remote, redaction) |
| I1b | `bin/a2a-bridge/src/main.rs` (terminal arms), `ctl/tweak.rs` / `ctl/implement.rs` (finalizer seam), `custody_push.rs` (snapshot) |
| I2 | `bin/a2a-bridge/src/main.rs` (review persistence), new `bin/a2a-bridge/src/evidence.rs`, `config.rs`, shared platform-dir helper, `docs/adr/0042-…md` |
| I3 | `crates/bridge-controller/src/merge.rs` (hook and gate), the shared module for the `storage_reap_clones.rs` helpers, `bin/a2a-bridge/src/main.rs` (hook wiring), `AGENTS.md`, `docs/onboarding.md` |

## Spec Refs

- ADR-0042 (governing)
- ADR-0041 §2 (findings to close)
- ADR-0027 (merge)
- ADR-0026 (checkpoint and resume)
- ADR-0025 (operation lock)

## Commit Message

Per increment, for example: `feat(custody): push run branch before review and merge (ADR-0042 I1a)`.

## Spec-review ledger

**Round 1 (gpt-6.1-sol, high): REJECT.** Numbering follows the first findings list in the review output. All findings folded except #8.

| # | Finding | Disposition |
|---|---|---|
| 1 | Side branch or tag lost | Folded: I3 check 4 |
| 2 | Submodule history lost | Folded: I3 check 5 |
| 3 | `showUntrackedFiles=no` hides untracked files | Folded: I1b §3 |
| 4 | Archive inside the clone | Folded: I2 §3 and I3 check 3 |
| 5 | `*.tmp` sweep catches foreign files | Folded: I2 §5 |
| 6 | Token in the checkpoint | Folded: common transport rules |
| 7 | Submodule push and URL rewrite | Folded: common transport rules |
| 8 | Agent work left unstaged and discarded | **Deferred**: existing design, outside ADR-0042 |
| 9 | Equal-SHA first push | Folded: I1a §3 |
| 10 | Relative URL | Folded: common transport rules |
| 11 | Resume skips a push | Folded: I1a §2 |
| 12 | Terminal arms skip the archive | Folded: I1b §1 |
| 13 | CI claim | Fixed in Facts |
| 14 | Checkpoint contract | Folded: I1a §4 |
| 15 | Temporary index | Folded: I1b §2 |
| 16 | Timeouts | Folded: common transport rules |
| 17 | Hook and ordering | Folded: I3 §1 |
| 18 | Archive acceptance | Folded: I3 check 3 |
| 19 | Accounting | Folded: I2 §4 |
| 20 | Concurrent sweep | Folded: I2 §5 |
| 21 | Review identity | Folded: I2 §1 |
| 22 | Test precision | Folded: I3 tests |
