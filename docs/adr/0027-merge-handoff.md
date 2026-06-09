# ADR-0027 — Merge hand-off (`a2a-bridge merge <id>`, Mode A)

**Date:** 2026-06-09
**Status:** Accepted

**Builds on:** ADR-0026 (resume — `resolve_clone`/`load_checkpoint`/`ImplementCheckpoint`), ADR-0019 (B2b-1 —
host-commits + the commit pin set + bot-identity-pre-merge), ADR-0025 (concurrent runs — flock lease + labels).

**Spec/plan:** `docs/superpowers/specs/2026-06-08-merge-handoff-design.md` (v6),
`docs/superpowers/plans/2026-06-08-merge-handoff.md`.

---

## Context

`a2a-bridge implement <task>` lands an agent's work as one bot-authored commit on a `branch` in a private
quarantine clone (`<allowed_cwd_root>/.a2a-implement/<id>`), over the run's `base_commit`, then prints a manual
hand-off (`implement::handoff_text`) for the operator to integrate. That last mile was manual. We want
`a2a-bridge merge <id>` (+ an `implement --merge` sugar) to integrate an **Approved** run's commit into its
`source_repo` **re-authored to the operator**, **without touching the operator's working checkout**, and
**safely under concurrent authors** — with CI-branchable exit codes.

## Decision

**Re-author the clone's commit and land it with a lease — no worktree, no lock.**

1. **`git commit-tree`** the clone's `current_commit` tree over `base_commit`, author **and** committer set to the
   operator via explicit `GIT_AUTHOR_*`/`GIT_COMMITTER_*` env (same `T` for both dates), reusing the identity-free
   commit pin prefix extracted from `commit_argv`. This creates a fresh commit object **without moving the
   clone's branch** — a failed push leaves the clone pristine, so retry is safe and the `head_sha ==
   current_commit` preflight still holds.
2. **`git push --force-with-lease=refs/heads/<target>:<base_commit>`** into `source_repo`. The lease
   fast-forwards `target` from `base_commit` to the re-authored commit **iff** `target` is still at
   `base_commit`. The receiving side is atomic, so **the lease IS the concurrency CAS** — concurrent pushes to
   one target: one wins, the rest get a stale-lease rejection (`StaleLease`). No external lock.
3. **Reap the clone on success** (path-guarded, never a bare `rm -rf`); on any failure keep it + print a targeted
   recovery line.

**Scope: Mode A only** (`--onto <branch>`, a fast-forward off `base_commit`). If the target advanced past
`base_commit` since the clone, merge **refuses** (`StaleLease`) rather than rewriting — nothing is lost, the
operator re-runs off the moved target. **Mode B** (`--as-branch`, parallel staging branches) is a deferred
fast-follow — all three regressions the review rounds found clustered in that surface, so Mode A ships first.

**The "no-touch the operator's checkout" guarantee has two layers:** a best-effort early refusal (the bridge
reads `source_repo`'s checked-out branch; if non-bare and == target, refuse before pushing) and the **atomic
backstop** — git's default `receive.denyCurrentBranch=refuse` refuses a push to any checked-out branch (main or
linked worktree), covering the preflight→push TOCTOU. A `source_repo` deliberately set to
`receive.denyCurrentBranch=updateInstead`/`ignore`/`warn` is **out of scope** (documented, not defended).

`merge` runs **no agent**: no run lease / RunHandle / recover_orphans / registry / policy / warm session. Its only
side effects are the clone-local `commit-tree`, the push, and the guarded reap. Failure classification reads the
source's post-failure ref state — **never parses push stderr**.

## Alternatives considered

- **Worktree + cherry-pick + a CAS temp-ref + per-target lock** (the v1 design) — rejected: it needs a worktree
  to host an index, a `cas_advance` temp-ref dance, and a per-target lock; `--force-with-lease` collapses all of
  that into one atomic push. (v2 also fixed a silent checkout-corruption blocker the worktree path had.)
- **`git merge`** — creates merge bubbles and doesn't re-author. **`format-patch`/`am`** — loses 3-way fidelity.
  **`commit --amend`** — moves the clone's branch, breaking retry-safety. **`git bundle`** — a cross-host
  transport seam, unnecessary for the same-host local push.

## Consequences

- **Exit codes** for CI: `0` merged; `1` usage/config/preflight (bad args, schema mismatch, source gone, clone
  preflight, operator unset); under `--merge` `2` the run did not reach Approved; `3` Approved but the merge could
  not land (StaleLease / target checked out).
- **`implement --merge`** is enabled by a new typed terminal outcome from `run_warm_loop` (it returns its
  `ImplementPhase`); plain `implement` exit behavior is unchanged. Approved-only (no `--force` — `implement` has
  none).
- **Operator identity** comes from `source_repo`'s git `user.name`/`user.email`, or a `[merge]`
  `author_name`/`author_email` override (both-or-neither); fail-loud if unset.
- **Concurrency caveat:** `merge` takes no run lease, so it must not run concurrently with `resume`/`merge` on the
  SAME `<id>` (a partial guard — the clone-HEAD preflight — exists; a first-class per-`<id>` advisory lock is
  deferred). Under genuine concurrency, target-moved (`StaleLease`) is the common case and each collision
  discards an Approved run's cost; the replay seam is kept open (clone retained, `commit-tree` pristine) but
  auto-replay is a follow-up. Mode A is not production-resilient under heavy concurrency.
- **Review provenance:** brainstormed, then 4 dual `spec-review` rounds (codex rigor + claude soundness) on the
  bridge's own containerized `spec-review`, plus a dual `plan-review`; the regression-prone Mode B was deferred.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
