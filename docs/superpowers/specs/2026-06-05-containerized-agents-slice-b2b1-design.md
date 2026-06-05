# Containerized Agents — Slice B2b-1 Design: the `implement` clone+edit+commit foundation

**Date:** 2026-06-05
**Status:** Draft (rev2, post dual-review). Folds the containerized-dogfood + a2a-local-codex spec reviews
(needs-changes): the uid/`safe.directory` round-trip posture, the full hook defense, the HEAD-guard, the
untracked stage check, the `Completed`-gate, the message-file channel, dropping `add -A`, and the
hand-off command fix.
**Builds on:** B2a (per-turn `:rw` `ContainerRwBackend`, merged 68ae7ba, ADR-0018). First sub-slice of B2b.
B2b-2 = build+test verify (toolchain image); B2b-3 = review-the-diff + APPROVE/REJECT.

## Goal

Ship the **foundation of the `implement` loop**: an `a2a-bridge implement` subcommand that creates a
per-task **quarantined git clone**, has the `impl` (`ContainerRw`) agent **edit + stage** a change inside
it, the **bridge deterministically commits** the agent-staged index on a task branch, and the clone is left
for the **operator to review/merge/reap** — human-approval entirely outside the bridge. Validated by a real
`implement/<task-id>` branch with the agent's staged change, the source repo untouched, the turn contained.
**No** build/test verify (B2b-2), **no** review-the-diff/approval primitive (B2b-3), reader image only.

## Decisions locked (design dialogue + dual-review fold)

1. **New `implement` subcommand** owns the clone lifecycle (the run-context) — NOT the executor/backend. It
   reuses the shipped `ContainerRw` `impl` agent + the executor (a 1-node `implement-edit` workflow) + B2a's
   `--session-cwd` threading. Essentially `run-workflow` wrapped with **clone-before** + **commit-and-
   hand-off-after**.
2. **Host-commits, "soft gate":** the agent owns the commit **content** — what to `git add` (judgment, incl.
   new files) and the commit **message**; the bridge owns the commit **action** (the deterministic per-turn
   checkpoint). The agent does NOT run `git commit`. The bridge does the mechanical commit (saves agent
   tokens). Works because the identical-path `:rw` mount means the agent's in-container `git add` writes the
   clone's `.git/index` on the host fs, and the host's `git commit` snapshots that same index — paths agree.
3. **Agent owns staging — NO `git add -A` fallback** [review fold]. If the agent staged nothing but the tree
   is dirty (unstaged/untracked), the bridge **flags it + does NOT commit + leaves the clone** for the
   operator (nothing is lost — the clone persists). Auto-`add -A` would override the agent's judgment and
   poison B2b-3's review-the-diff. (Warm-slice enhancement: re-prompt the warm agent to stage/clean.)
4. **Message via a file channel, not prose parsing** [review fold]. The agent writes the commit message to
   `<clone>/.git/A2A_COMMIT_MSG` (untracked — inside `.git`, never committed); the bridge reads it off the
   host fs (same structured channel as the staged index). Task-derived fallback if absent/empty/whitespace.
5. **Runtime posture: podman preferred, Docker Desktop supported; rootful-Docker-on-Linux out of scope**
   [review fold — the load-bearing item]. The host-commit round-trip crosses an ownership boundary
   (in-container writes vs host commit). **rootless podman** (preferred) maps container-root→host-user, and
   **Docker Desktop** remaps via its file-sharing layer — both make the clone host-owned, so the host commit
   works and git's "dubious ownership" guard doesn't fire. **Rootful Docker on native Linux is out of scope**
   (root-owned files; host commit can't write `.git`). All host git ops additionally pass `-c
   safe.directory=<clone>` defensively. The live macOS gate can't prove the Linux story — a **rootless-podman
   Linux spike** is the real Linux validation (deferred).
6. **`--no-hardlinks` committed-only clone** (CopyMode=committed; dirty-tree deferred). Independent object
   store; **the source repo is never mounted** into any container → "source untouched" is an *enforced
   invariant*, not convention.
7. **Bot identity pre-merge, operator identity at merge.** Bridge commits with `a2a-implement
   <implement@a2a-bridge.local>` (rewritable pre-merge); the operator re-authors as themselves at merge.

## Architecture

### Entry point
```
a2a-bridge implement <task> --repo <path> [--base-ref <ref>] [--config <path>] [--workflow <id>]
```
- `<task>`: the task description (positional, templated as `{{input}}` into the edit prompt).
- `--repo`: the source repo to clone (any host path — `git clone` runs on the host; only the *clone* lands
  under `allowed_cwd_root`).
- `--base-ref`: ref to clone from. **Default = the source repo's current HEAD commit** (resolved to a SHA
  for determinism). Detached HEAD → that commit; a missing/invalid ref → loud error; a dirty source worktree
  is irrelevant (committed-only clone). 
- `--config` (the bridge config: the `impl` `ContainerRw` agent + the `implement-edit` workflow);
  `--workflow` (default `implement-edit`).
- Dispatched in `main.rs` alongside `serve|run-workflow|submit|task|init`. **All host git ops are direct
  argv `std::process::Command` calls — no shell, no string interpolation** (repo paths / branch / message
  passed as argv).

### Clone lifecycle (run-context owns it)
1. Read + canonicalize `allowed_cwd_root` from the config — **consistently with `ContainerRwBackend`'s
   rw-target canonicalization** (resolve symlinks). Fail loudly on missing/non-directory root, permission
   errors, or symlink escape.
2. Generate `task-id = impl-<pid>-<nonce>` (nonce = lowercase-alnum, fixed length, filesystem- + branch-
   name-safe); **retry** if the clone dir or `implement/<task-id>` branch already exists.
3. **Assert the clone dest is NOT inside another git worktree** (incl. the source repo) — `git -C
   <root>/.a2a-implement rev-parse --is-inside-work-tree` must fail; else refuse (cloning into a repo dirties
   it). Create `<root>/.a2a-implement/` (fail loudly if it can't be created).
4. `git clone --no-hardlinks <repo> <root>/.a2a-implement/<task-id>` → `checkout <base-ref-SHA>` →
   `checkout -b implement/<task-id>`. The clone dir is `is_under` the canonical `allowed_cwd_root` (the
   `ContainerRw` mount anchor, B2a `resolve_rw_target`).
5. The clone **persists** after the run (operator reaps on merge).

### Edit turn (reuse the executor)
A 1-node `implement-edit` workflow (`agent="impl"`, `prompt_file="../prompts/implement-edit.md"`,
`inputs=[]`). The subcommand **snapshots `git -C <clone> rev-parse HEAD`** (pre-turn), then runs
`executor.run_with_context(graph, input=<task>, …, ctx{session_cwd: Some(<clone>)})`. The agent, in the
`:rw` clone (identical-path):
- edits files; uses `git diff` / `git stash` as working tools;
- **stages** its change (`git add <paths>`, incl. new files — judgment);
- **writes the commit message to `.git/A2A_COMMIT_MSG`**;
- does **NOT** run `git commit` and does **NOT** switch branches.

### Commit (bridge, deterministic state machine)
After the workflow stream terminates:
1. **Settle the container:** `retire()` the backend (awaited) so no container races the host commit on the
   `.git/index` lock [review: B2a reaps async]. Clear any stale `.git/index.lock` if present.
2. **Outcome gate:** commit ONLY on `WorkflowOutcome::Completed`. On executor error / Failed / Canceled →
   **no commit**, leave the clone for inspection, report.
3. **HEAD guard:** assert `git -C <clone> symbolic-ref --short HEAD == implement/<task-id>` and that HEAD
   matches the pre-turn snapshot. If the agent **switched branch** → error (leave clone). If HEAD
   **advanced** (the agent committed despite the contract) → report it + leave the clone for the operator to
   re-author (don't double-commit).
4. **Stage classification** via `git -C <clone> status --porcelain` (detects untracked, unlike `git diff
   --quiet`):
   - staged changes present → commit them.
   - nothing staged but tree dirty (unstaged/untracked) → **flag + no commit + leave clone** (Decision 3).
   - nothing changed at all → no commit, report "implement made no changes."
5. **Message:** read `<clone>/.git/A2A_COMMIT_MSG`; if absent / empty / whitespace-only / invalid-UTF-8 /
   oversized → **task-derived fallback** (`implement: <first line of task, truncated>`) + flag. Strip the
   file before/after committing (it's in `.git`, never tracked).
6. **Commit (direct argv):**
   `git -C <clone> -c safe.directory=<clone> -c core.hooksPath=/dev/null -c commit.gpgsign=false
   -c user.name=a2a-implement -c user.email=implement@a2a-bridge.local commit --no-verify -m "<message>"`
   — `--no-verify` alone is insufficient (`prepare-commit-msg`/`post-commit` still run; the agent can set
   `core.hooksPath`), so hooks are neutralized via `core.hooksPath=/dev/null` and signing is disabled.
7. **Report leftover** uncommitted changes after the commit (so the operator knows what the agent left
   unstaged — it persists in the clone, not in the commit).

### Hand-off (human-approval = outside the bridge)
Print the clone path, branch `implement/<task-id>`, commit sha + subject, and the **operator re-author +
merge** command (corrected — `cherry-pick` has no `--reset-author`):
```
git -C <repo> fetch <clone> implement/<task-id>
git -C <repo> cherry-pick -n FETCH_HEAD && git -C <repo> commit -C FETCH_HEAD --reset-author
rm -rf <clone>          # reap
```
The text is **informational** (target repo should be clean; conflicts are operator-handled). No in-bridge
APPROVE/REJECT (B2b-3). Bot commits are rewritable; the operator owns final authorship.

## Component / file boundaries

| Concern | Home | Note |
|---|---|---|
| `implement` subcommand dispatch + orchestration (clone → run → commit state machine → hand-off) | `bin/a2a-bridge/src/main.rs` | `Some("implement") => implement_cmd(…)` |
| **Pure helpers** (git argv builders, msg-file reader + fallback, task-id, hand-off text) | **`bin/a2a-bridge/src/implement.rs`** (new) | git-free + Docker-free unit-testable |
| **stage classifier + commit round-trip** | `implement.rs` | needs **temp-repo git tests** (not pure) |
| `implement-edit` workflow + prompt | `examples/a2a-bridge.containerized.toml` + `prompts/implement-edit.md` | 1 node, `agent="impl"`; edit+stage+write-msg-file contract |
| reuse | `bridge-workflow::executor` (run_with_context + Completed outcome), B2a `ContainerRw` `impl` agent | no new backend / crate / image |

## Testing
- **Unit, git-free, in `implement.rs`:** clone/checkout/commit argv builders (`--no-hardlinks`,
  `--no-verify`, the `-c safe.directory`/`core.hooksPath`/`gpgsign`/`user.*` pins); the
  `.git/A2A_COMMIT_MSG` reader + fallback (absent/empty/whitespace/oversized → task-derived); task-id shape
  + collision retry; the hand-off text (paths/branch/sha + the corrected re-author command).
- **Unit, temp-repo git tests** [review: not pure]: the stage classifier (`status --porcelain` →
  staged / dirty-unstaged / clean); the HEAD guard (branch-switch + HEAD-advance detection); the full
  host-commit round-trip on a temp repo (the `-c` pins applied; bot identity on the commit).
- **Live gate (Docker Desktop / podman, operator-run):** `a2a-bridge implement "create FOO.md containing
  BAR, stage it, and write the commit message to .git/A2A_COMMIT_MSG" --repo <throwaway clone of this repo>`
  → a real `implement/<id>` branch with the agent's staged change committed (bot identity) + the agent's
  message; the **source repo is untouched** (`git -C <repo> status` clean, no new branch); `docker events`
  shows the contained `a2a-rw-*` turn; the empty-staging path (a prompt that edits but doesn't stage) →
  flag + no commit + clone left.
- Coverage after `cargo llvm-cov clean --workspace` (floors workspace 85, bridge-core 90, bridge-workflow
  90) — the live Docker gate + the unit/temp-repo tests are the mandatory acceptance.

## Security (B2b-1 surface)
- **Quarantine invariant:** the source repo is **never mounted** into any container — only the clone is
  (`ContainerRw` mounts the per-session cwd). "Source untouched" is enforced by the mount model.
- **Hooks:** all host git ops in the clone use `-c core.hooksPath=/dev/null` (+ `--no-verify`,
  `commit.gpgsign=false`) so an agent-planted hook or `core.hooksPath` can't execute on the host. Hooks
  aren't tracked → a merge never carries them; they die with `rm -rf <clone>`.
- **HEAD guard** (above) prevents the agent committing on the wrong branch / under a non-bot identity going
  unnoticed.
- **No shell:** direct argv for every host git call (paths/branch/message are argv, not interpolated).
- **Residual** (accepted for B2b-1, noted): repo-local git config / attributes / templates in the clone are
  not fully sanitized beyond the `-c` pins above — a quarantine risk bounded by the reaped clone + operator
  review.

## Deferred (later sub-slices / follow-ons)
- **B2b-2:** build+test **verify** node (Rust **toolchain image** + cargo-under-egress-lockdown).
- **B2b-3:** **review-the-diff** lenses (existing review workflow on `git diff base..HEAD`) → synth verdict →
  **APPROVE/REJECT**; the review→tweak loop (per-turn host commits = checkpoints).
- **Warm-slice:** on empty-but-dirty, **re-prompt the warm agent** to stage/clean (cheap on a warm session);
  warm re-ask for a missing message.
- **Native-Linux (rootless podman) spike** of the uid/`safe.directory` round-trip; CopyMode=dirty-tree;
  orphan-clone `implement --list`/`--reap-stale` (or print existing clones on invoke); detachable
  `implement` (serve/TaskStore) + formal crash-resume (`reset --hard HEAD` + re-run); gVisor `--runtime=runsc`.

## Firewall
Designed from the bridge's own ports (the `implement` subcommand site, `executor.run_with_context` +
`WorkflowRunContext.session_cwd`, the B2a `ContainerRw` backend + `resolve_rw_target`, the review-workflow
`{{input}}` convention). `tsk-tsk` READ as prior art for the repo-copy/CopyMode shape only — NOT adopted.
Dual review = containerized dogfood PRIMARY + a2a-local `codex-review` (gpt-5.5) backstop. Once `implement`
ships it should build its own subsequent changes (the ultimate dogfood).
