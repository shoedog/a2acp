# ADR-0019 — The `implement` clone+edit+commit Foundation (Containerized Agents, Slice B2b-1)

**Date:** 2026-06-05
**Status:** Accepted

**Builds on:** ADR-0018 (B2a — per-turn `:rw` `ContainerRwBackend`). B2a delivered a write-capable container
per turn; B2b-1 builds the first rung of the actual coding loop on it. First sub-slice of B2b (B2b-2 =
build+test verify + toolchain image; B2b-3 = review-the-diff + APPROVE/REJECT).

---

## Context

A coding agent that edits a real repo and produces a reviewable commit needs: a **quarantine** (so the
source repo is never at risk), a way for the agent to **stage** its change, a **deterministic commit**, and
a **human-approval** step. The riskiest, newest mechanic is the per-task clone lifecycle + a coding agent
committing into a quarantine + the operator merging — so B2b-1 isolates exactly that, reusing the shipped
`ContainerRw` agent + the reader image (no build/test verify, no review/approval primitive — those are
B2b-2/B2b-3).

## Decision

A new `a2a-bridge implement <task> --repo <path>` subcommand that owns the clone lifecycle (the
run-context, NOT the executor/backend): it clones, runs a 1-node `implement-edit` workflow on the
`ContainerRw` `impl` agent with `session_cwd` = the clone, then runs a deterministic commit state machine
and prints an operator hand-off.

- **Host-commits "soft gate":** the agent owns the commit **content** — what to `git add` (judgment, incl.
  new files) and the commit **message** (written to `.git/A2A_COMMIT_MSG`, a structured file channel, not
  prose-parsed); the bridge owns the commit **action** (the deterministic per-turn checkpoint). The agent
  does NOT run `git commit`. This works because the identical-path `:rw` mount means the agent's
  in-container `git add` writes the clone's `.git/index` on the host fs, and the host's `git commit`
  snapshots that same index.
- **Agent owns staging — NO `git add -A`.** Empty staged index + dirty tree → flag + **no commit + leave
  the clone** (nothing lost — the clone persists for the operator). Auto-`add -A` would override the agent's
  judgment and poison B2b-3's review-the-diff. (The warm-pool slice will re-prompt the warm agent instead.)
- **The commit is hardened:** `git -c safe.directory=<clone> -c core.hooksPath=/dev/null
  -c commit.gpgsign=false -c user.name=a2a-implement -c user.email=… commit --no-verify` — `--no-verify`
  alone is insufficient (prepare-commit-msg/post-commit still run, and the agent can set `core.hooksPath`),
  so hooks are neutralized via `core.hooksPath` too; `safe.directory` pins the container-root→host
  ownership round-trip; bot identity is pre-merge.
- **Bot identity pre-merge, operator identity at merge.** The hand-off prints the corrected re-author +
  merge command (`cherry-pick -n FETCH_HEAD && git commit -C FETCH_HEAD --reset-author`) so contributors
  know the responsible human. No in-bridge APPROVE/REJECT (B2b-3).
- **Pure soft-gate `decide()`** (the plan-review keystone): the whole decision — `Completed`-gate, HEAD
  guard (the agent has `:rw`+git and could switch branch / commit itself), stage classification (`git
  status --porcelain`, which detects untracked where `git diff --quiet` misses), message resolution — is a
  pure function over a small input struct, unit-tested as a matrix; `implement_cmd` resolves inputs +
  executes the chosen `Action ∈ {Commit | NoCommitDirty | NoCommitClean | Abort}`.
- **Quarantine = `git clone --no-hardlinks`** under `allowed_cwd_root/.a2a-implement/<task-id>` (an
  independent object store; `is_under` the `ContainerRw` mount anchor; clone-dest-not-inside-a-worktree
  guarded; `--base-ref` resolved to a SHA). The **source repo is never mounted** → "source untouched" is an
  enforced invariant. The clone persists for the operator; reaped on merge.
- **`make_spawn_fn` extracted** so run-workflow and `implement` share one registry-build (no drift).

## Consequences

- **Settle, ratified (vs the spec's first draft):** an awaited `retire()` would need a new `Registry`
  retire-all-and-await seam (`retire()` is per-`AgentBackend`); instead the host commit relies on the
  agent's in-container git completing before the turn's `Done` (releasing the index lock) + a bounded
  retry-on-`index.lock` (clearing a stale lock only after retries). Documented in the spec.
- **Runtime posture:** podman (preferred) and Docker Desktop both map container-root→host-user, so the
  host commit works; rootful-Docker-on-Linux is out of scope; a rootless-podman Linux spike is deferred.
- **Per-turn:** B2b-1's edit is one turn; the review→tweak loop is B2b-3 (per-turn host commits =
  checkpoints).

## Validation

`implement.rs` helpers (argv pins, message-file reader + fallback, task-id, hand-off, the `decide` matrix)
git-free; the impure ops (stage classifier, HEAD guard, host-commit with a planted `pre-commit` +
`core.hooksPath` + `prepare-commit-msg` that must NOT fire, clone-dest guard, source-untouched) via
temp-repo git tests. Docker-free config-validation (the `impl` agent + `implement-edit` parse + validate).
**Live gate (PASS):** `implement "create FOO.md…"` against a throwaway repo → `FOO.md` (content `BAR`)
committed on `implement/<id>` under the bot identity with the agent's message; **source repo untouched**;
`docker events` show the contained `a2a-rw-*` `start`→`die`→`destroy`. Workspace coverage 88.87% region /
90.01% line; `implement.rs` 93%; clippy `-D warnings` clean.

## Alternatives considered

- **Agent commits in-container:** rejected — host-commits gives deterministic per-turn checkpoints + saves
  agent tokens; the agent still owns staging (content) + the message via the file channel.
- **Reuse `run-workflow` with the operator pre-cloning:** rejected — the clone lifecycle (create/persist/
  hand-off) needs an integrated home; the `implement` subcommand owns it.
- **`git add -A` recovery fallback:** rejected — overrides the agent's staging judgment; leave the clone
  for inspection instead.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
