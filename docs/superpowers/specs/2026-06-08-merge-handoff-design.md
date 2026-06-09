# `a2a-bridge merge <id>` — Design Spec (v2, post spec-review)

**Date:** 2026-06-08
**Status:** Approved (brainstorm). Plan + ADR-0027 to follow.
**Builds on:** ADR-0026 (resume — `resolve_clone`/`load_checkpoint`/`ImplementCheckpoint`), ADR-0019
(B2b-1 — host-commits + the `commit_argv` pin set + bot-identity-pre-merge), ADR-0025 (concurrent runs).
**Reviewed by:** the bridge's own clean-room `design` workflow (codex+claude) AND a dual `spec-review`
(codex *rigor* + claude *soundness*). **v2 adopts claude's push-based redesign** — it removes the detached
worktree, `WorktreeGuard`, `.a2a-merge/` dirs, `cas_advance`/`update-ref`, the `refs/a2a/merge/<id>` temp
ref, **and** the per-target lock, while fixing the BLOCKER that the worktree path could silently corrupt the
operator's checkout.

---

## Goal

Automate the manual merge hand-off (`implement::handoff_text`) as **`a2a-bridge merge <id>`** (+ an
`implement --merge` sugar), integrating an `Approved` run's commit into its `source_repo` **without touching
the operator's working checkout** and **safely under concurrent authors**.

## Why push-based (the v1→v2 change)

The clone is already a private, single-author repo whose `branch` holds exactly one commit (`current_commit`,
bot-authored) over `base_commit`. So we don't need a worktree to host an index:

1. **Re-author with `git commit-tree`** (NOT `commit --amend`): in the clone, with BOTH the author AND the
   committer set to the operator via explicit env (so `commit-tree`'s committer can't fall back to the
   ambient git config) plus the host-commit pins:
   ```
   GIT_AUTHOR_NAME=<OP> GIT_AUTHOR_EMAIL=<OP> GIT_COMMITTER_NAME=<OP> GIT_COMMITTER_EMAIL=<OP> \
     git -C <clone> -c safe.directory=<clone> -c core.hooksPath=/dev/null -c commit.gpgsign=false \
         commit-tree <current_commit^{tree}> -p <base_commit> -m <original_message>
   ```
   → a new commit object, **author == committer == operator** with FRESH author/committer dates (a clean
   re-authorship, not a preserved bot date), same tree, parent `base_commit`, **without moving the clone's
   branch** (so a failed push leaves the clone pristine → retry-safe; `commit --amend` would move the branch
   and break the `head_sha == current_commit` preflight on retry).
2. **Push it** from the clone to `source_repo`:
   `git -C <clone> push <source_repo> <reauthored>:refs/heads/<target> --force-with-lease=refs/heads/<target>:<base_commit>`.
   - `--force-with-lease=<target>:<base_commit>` **IS the CAS**: the push fast-forwards `target` from
     `base_commit` to `reauthored` ONLY if `target` is still at `base_commit`. If `target` moved → lease
     fails → **refuse** (the v1 "CAS-stale → refuse" decision). Atomic on the receiving side → **no external
     lock needed** (concurrent pushes to one target: one wins, the rest get a stale-lease rejection).
   - Pushing to a **checked-out** branch is refused by git's default `receive.denyCurrentBranch` — so the
     worktree path's silent-checkout-corruption BLOCKER becomes a **safe, git-native refusal** (surfaced as
     "target is checked out — switch off it or pick another target").
3. **Reap the clone on success**; on any failure keep it + print a recovery command (NOT `rm -rf`).

Nothing is created in `source_repo` except the atomic ref update — so the worktree/ref-leak concerns
(needing `git worktree prune`) **evaporate**.

**Integration-approach comparison (recorded):** push-`commit-tree` beats cherry-pick-in-a-worktree (no
worktree/lock/CAS-ref/temp-ref machinery, force-with-lease is the CAS, denyCurrentBranch is free safety),
beats `git merge` (no merge bubbles, re-authors), beats `format-patch`/`am` (3-way fidelity, no `.rej`).
`git bundle` is a *transport* (cross-host), kept as a deferred seam — `do_clone` is same-host so a local
push suffices.

## Two modes (selectable by work pattern)

Both run the SAME phase gate (`decide_merge`) first; they differ only in **where** `reauthored` is pushed.

- **Mode A — `--onto <branch>` (DEFAULT).** Push to `refs/heads/<target>` with the `base_commit` lease
  (fast-forward an accumulating line). For *sequential* tasks across slices.
- **Mode B — `--as-branch [<name>]`.** Push to a **new** `refs/heads/<name>` (default `implement/<task_id>`,
  which is unique per run); **refuse if the branch already exists** unless `--force` (a fresh staging
  branch). No lease/CAS. For *parallel* tasks in one slice. Also operator-re-authored (no "deferred
  re-author" promise).

When neither flag is given → **mode A onto the resolved target**.

## The gate — applies to BOTH modes (fixes the v1 "mode B bypasses the gate" bug)

`decide_merge` runs before either landing:
- `phase == Approved` → `Merge`.
- `phase == LoopStopped` (finished, not approved) → `Refuse` unless `--force`.
- `phase ∈ {Cloned, EditStarted, FirstCommitCreated, InLoop}` (not finished) → **`RefuseHard`** — `--force`
  cannot override ("not finished — `resume` it first"). *(Mode B must NOT short-circuit before this — the v1
  bug published empty/unconverged branches.)*
- `current_commit == None` (no commit exists) → `RefuseHard`. *(The checkpoint stores
  `current_commit: Option`; an `Approved` run always has `Some`, but refuse defensively.)*

## Components & file boundaries

| File | Change |
|---|---|
| `bin/a2a-bridge/src/merge.rs` | **NEW** — pure gate (`MergePlan`/`decide_merge`/`resolve_target`) + impure git ops (`operator_from`, `reauthor_commit`, `push_landing`), mirroring `implement_resume.rs` (pure-tested + temp-repo-tested, docker-free). |
| `bin/a2a-bridge/src/main.rs` | `mod merge;`; `merge_cmd` + the `merge` dispatch arm; `run_warm_loop` returns a typed terminal outcome so `implement --merge` calls `merge_run` **only on `Approved`**. |
| `bin/a2a-bridge/src/config.rs` | optional `[merge]` block (`MergeToml`/`MergeConfig`), fail-loud pre-flight parse like `ImplementToml`. |
| `bin/a2a-bridge/src/implement.rs` | **extract the git-config pin prefix** from `commit_argv` into a shared, identity-parameterized helper (`commit_argv` hardcodes `BOT` + `-m`; `reauthor_commit` needs the same `safe.directory`/`hooksPath=/dev/null`/`gpgsign=false` pins with the OPERATOR identity + `commit-tree`). Both call the shared prefix. |

`merge` runs **no agent**: it must NOT touch the run lease / `RunHandle` / `recover_orphans` / `RunEndGuard`
/ registry / policy / warm session. Its only side effects are the clone-local `commit-tree`, the push, and
the on-success clone reap.

## Pure core (unit-tested, git-free)

```rust
pub enum MergePlan {
    Merge { target: String, mode: Mode },
    Refuse(String),     // recoverable: LoopStopped w/o --force; unresolvable target
    RefuseHard(String), // non-terminal phase or current_commit==None — --force CANNOT override
}
#[derive(Clone, Copy)] pub enum Mode { Onto, AsBranch }

/// Returns a validated SHORT BRANCH NAME (e.g. `main`, `feature/x`) — NEVER a full ref. The single
/// `refs/heads/{branch}` is constructed ONLY at the git boundary (`push_landing`), so `MergePlan.target`,
/// `push_landing.dst_branch`, the config, and the output text all carry the SAME short-name representation
/// (avoids `refs/heads/refs/heads/main`).
/// Precedence: --onto > [merge].target_ref > checkpoint.base_ref. None ⇒ Err ("pass a target / --onto").
/// Validation = `git check-ref-format --branch <name>` semantics (pure equivalent): reject empty,
/// `HEAD`, raw SHAs (40-hex), a `refs/…` prefix, `refs/remotes/*` / `origin/*`, tags, `..`, a trailing
/// `/` or `.lock`, a leading `-`, and any space/control char. `base_ref` from a checkpoint is normalized
/// the same way (it is already a branch name).
pub fn resolve_target(cli_onto: Option<&str>, cfg: Option<&str>, base_ref: Option<&str>)
    -> Result<String, String>;

pub fn decide_merge(phase: ImplementPhase, has_commit: bool, force: bool,
                    target: &Result<String, String>, mode: Mode) -> MergePlan;
```
`decide_merge` matrix (mode-independent): `Approved`+`has_commit`→`Merge`; `LoopStopped`→`Refuse` unless
`force`; non-terminal **or** `!has_commit`→`RefuseHard`; `target` Err→`Refuse`.

## Impure ops (temp-repo tested, docker-free)

```rust
pub struct OperatorIdent { name: String, email: String }
/// source_repo git config user.name+user.email (or [merge] override). FAIL LOUD if EITHER half is missing
/// (no committing as nobody on headless/CI). A config override must supply BOTH or it's a parse error.
pub fn operator_from(repo: &Path, cfg_override: Option<&OperatorIdent>) -> Result<OperatorIdent, String>;

/// commit-tree the implement commit's tree over base_commit as the operator → the re-authored sha. Does NOT
/// move the clone's branch (retry-safe). Reuses the extracted commit pin prefix.
pub fn reauthor_commit(clone: &Path, current_commit: &str, base_commit: &str, msg: &str, op: &OperatorIdent)
    -> Result<String, String>;

pub enum PushError { StaleLease, CheckedOutTarget, BranchExists, Other(String) }
/// Push `reauthored` from the clone into source_repo. `dst_branch` is a SHORT name; `push_landing` builds the
/// single `refs/heads/{dst_branch}` itself. The `intent` carries the mode-specific safety; there is NO bare
/// `force` param (so `--force` can never silently weaken a lease):
///   - `LandOnto { base }` (Mode A): always `--force-with-lease=refs/heads/{dst}:{base}` → FF iff the target
///     is still at `base`; else `StaleLease`. `--force` does NOT change this — it only flips the gate
///     (`LoopStopped`) earlier, never the lease.
///   - `CreateBranch` (Mode B, default): `refs/heads/{dst}` must NOT exist → else `BranchExists`.
///   - `ReplaceBranch { expect }` (Mode B + `--force`): `--force-with-lease=refs/heads/{dst}:{expect}` where
///     `expect` is the branch's CURRENT tip — a CHECKED replace, never an unconditional `+dst` overwrite (so
///     a concurrent writer is detected → `StaleLease`).
/// Pushing onto a checked-out branch in `source_repo` → git's `receive.denyCurrentBranch` → `CheckedOutTarget`.
pub enum PushIntent<'a> { LandOnto { base: &'a str }, CreateBranch, ReplaceBranch { expect: &'a str } }
pub fn push_landing(clone: &Path, source_repo: &Path, reauthored: &str, dst_branch: &str,
                    intent: PushIntent<'_>) -> Result<(), PushError>;
```

## Config

```toml
[merge]
target_ref   = "main"   # optional; CLI --onto wins over this
# operator identity override (optional; else source_repo git config; else fail loud). BOTH or neither.
# author_name  = "…"
# author_email = "…"
```
```rust
#[serde(default)] pub merge: Option<MergeToml>,
pub struct MergeConfig { pub target_ref: Option<String>, pub author: Option<OperatorIdent> }
```
(No `lock_wait_secs` — there is no lock in v2.)

## Command surface

```
a2a-bridge merge <id> [--config <path>] [--force] [--onto <branch> | --as-branch [<name>]]
a2a-bridge implement <task> --repo <path> … [--merge [--onto <branch>]]   # Approved-only mode-A sugar
a2a-bridge implement --resume <id> …       [--merge [--onto <branch>]]
```
`implement --merge` target selection: `--onto` if present, else `[merge].target_ref`, else `base_ref`;
`base_ref == None` (HEAD-based run) with no config target → fail loud. `--merge` only does mode A (no
`--as-branch` sugar — stage-as-branch is an explicit `merge <id> --as-branch` step).

## Control flow

```
merge_cmd(cfg, id, force, onto, as_branch):
  root  = canonicalize(allowed_cwd_root)?; clone = resolve_clone(root, id)?; ck = load_checkpoint(clone)?
  src   = canonicalize(ck.source_repo)  → Err ⇒ PreflightFail: "source repo {ck.source_repo} gone/moved —
          keep clone, exit nonzero" (the checkpoint persists the user-supplied path; a moved/deleted/non-git
          src is a non-overrideable refusal). NO [merge] override of the stored source.
  # CLONE PREFLIGHT (cheap, impure) — each failure is a NON-overridable refusal (force ignored), KEEP clone,
  # exit nonzero, with DISTINCT recovery text; this guards retry-safety so it must run before any push:
  #   current_branch(clone) != ck.branch        → "clone on wrong branch — inspect {clone}, do not merge"
  #   head_sha(clone)       != ck.current_commit → "clone HEAD moved off the checkpoint — re-run from a clean clone"
  #   is_worktree_dirty(clone)                   → "clone worktree dirty — a half-finished fix; inspect {clone}"
  mode  = if as_branch.is_some() { AsBranch } else { Onto }
  target = match mode { Onto => resolve_target(onto, cfg.target_ref, ck.base_ref.as_deref()),
                        AsBranch => validate_branch(as_branch.unwrap_or(format!("implement/{}", ck.task_id))) }
  match decide_merge(ck.phase, ck.current_commit.is_some(), force, &target, mode):
     Refuse(m)|RefuseHard(m) => eprintln(m) + exit nonzero (KEEP clone; no rm -rf in the text)
     Merge{target, mode}     => merge_run(cfg, ck, src, &target, mode, force, clone)

merge_run(cfg, ck, src, target, mode, force, clone):
  op  = operator_from(src, cfg.author.as_ref())?                # fail loud if unset (BOTH halves)
  rt  = reauthor_commit(clone, &ck.current_commit?, &ck.base_commit, &ck.original_message_or_task, &op)?
  intent = match mode {
     Onto       => PushIntent::LandOnto { base: &ck.base_commit },           # lease=base; --force NEVER weakens
     AsBranch if !branch_exists(src, target) => PushIntent::CreateBranch,
     AsBranch if force => PushIntent::ReplaceBranch { expect: &rev_parse(src, target) },  # CHECKED replace
     AsBranch   => /* exists && !force */ return BranchExists path,
  }
  match push_landing(clone, src, &rt, target, intent):
     Ok(())                => rm -rf clone; println!("merged {rt} into {target}")
     Err(StaleLease)       => keep clone; "‹target› moved since base — re-run `merge {id}` (or `resume` first)"
     Err(CheckedOutTarget) => keep clone; "‹target› is checked out in {src} — switch off it / pick --onto"
     Err(BranchExists)     => keep clone; "branch ‹target› exists — pick a name or pass --force"
     Err(Other(e))         => keep clone; "merge failed: {e}; clone kept at {clone}"
```
Exit non-zero on any `Err`/`Refuse`/preflight failure. `ck.current_commit`/`original_message` are `Option` —
the gate refuses `current_commit==None`; `original_message==None` falls back to a task-derived subject.

## Resolved decisions (carried from v1, still valid)

1. **CAS-stale → refuse** (the `force-with-lease` rejection is the refusal); bounded retry-replay (cherry-pick
   the changes onto the moved tip in the clone, then push) deferred.
2. **Transport: local push only.** Keep a `Transport` notion for `git bundle` cross-host, unexercised in v1.
3. **Operator identity:** `[merge]` override (BOTH halves) **and** fail-loud when neither config nor
   `source_repo` git config supplies it.

## Testing strategy

Pure core unit-tested; git ops over temp repos (docker-free); `merge_cmd` + `--merge` sugar live-gated.
- **`decide_merge`** — full phase × `has_commit` × force matrix; **keystones:** non-terminal+force →
  `RefuseHard`; `current_commit==None` → `RefuseHard`; mode does not change the gate.
- **`resolve_target`** — precedence, normalization, `None`→Err, reject HEAD/SHA/remote/tag.
- **`reauthor_commit`** — author==committer==operator (NOT bot); same tree as `current_commit`; the clone's
  branch is **unmoved** (retry-safe).
- **`push_landing`** over temp repos — mode A FF when `target==base_commit`; **StaleLease** when the target
  moved; **CheckedOutTarget** when the target is the checked-out branch (denyCurrentBranch); mode B creates a
  new branch; **BranchExists** refusal; `--force` overrides BranchExists.
- **`operator_from`** — sources repo git config; fail-loud when unset; `[merge]` override (both halves) wins;
  half-override → error.
- **concurrency** — two `push_landing` to ONE target over a temp repo: exactly one succeeds, the other
  StaleLease (no lock, force-with-lease is the CAS); two to DISTINCT targets both succeed.
- **source-unchanged invariant (testable)** — after a merge, `source_repo`'s `git rev-parse HEAD` is
  unchanged AND `git status --porcelain=v1 --untracked-files=all` is byte-identical to before (no index/
  worktree write); ONLY the pushed `refs/heads/<target>` may have moved.
- **clone preflight** — wrong-branch / moved-HEAD / dirty each refuse (force ignored), keep the clone, exit
  nonzero, with distinct messages; a gone/non-git `source_repo` likewise refuses.
- **`reauthor_commit` dates** — author date == committer date, both FRESH (not the bot commit's date).
- **Live gate** — operator-run: a real `Approved` run → `merge <id>` lands on the target re-authored, clone
  reaped; a `LoopStopped` run refuses without `--force`; merging onto the **checked-out** branch refuses
  cleanly; `--as-branch` lands a branch; two merges to distinct targets succeed in parallel.

## Build order (smallest shippable slices, docker-free until the live gate)

1. **Pin-prefix extraction** in `implement.rs` (`commit_argv` → shared identity-parameterized prefix) + its
   existing tests stay green.
2. **Pure core** — `MergePlan`/`Mode`/`decide_merge`/`resolve_target` + the full matrix tests.
3. **`reauthor_commit`** (commit-tree, retry-safe) + temp-repo tests.
4. **`push_landing`** (mode A FF + lease + denyCurrentBranch; mode B new-branch + exists-refusal) +
   temp-repo tests incl. the concurrency (two-push) test + source-unchanged invariant.
5. **`merge <id>`** — `merge_cmd` + dispatch + `[merge]` config (fail-loud parse, default tests) +
   `operator_from` fail-loud; clone reaped on success / kept on failure (recovery text, no `rm -rf`).
6. **`run_warm_loop` typed outcome + `implement --merge`** sugar (Approved-only, mode A).
7. **Bundle transport** — deferred seam.

## Risks

- **Operator identity unset on headless hosts** — fail-loud + `[merge]` override + an unset test.
- **`base_ref == None`** — `resolve_target` errs explicitly; `--onto`/`[merge].target_ref` make it turnkey.
- **Re-author idempotency** — `commit-tree` (not amend) leaves the clone unmoved, so a failed push is cleanly
  retryable; the preflight `head_sha == current_commit` still holds on retry.
- **Recovery text** — never reuses `handoff_text`'s `rm -rf "{clone}"`; each `PushError` prints a targeted,
  reap-free recovery line.

## Spec-review resolutions (codex rigor, v2→v3)

- **BLOCKER target double-wrap** → `resolve_target` returns a SHORT branch name; `refs/heads/{}` built only
  inside `push_landing`. One representation across `MergePlan.target` / `dst_branch` / config / output.
- **BLOCKER `--force` weakening the lease** → no bare `force` in `push_landing`; a `PushIntent` enum carries
  the mode-specific safety. Mode A is ALWAYS `LandOnto{base}` (lease unconditional); `--force` only flips the
  `LoopStopped` gate. Mode B `--force` = `ReplaceBranch{expect=current tip}` (a CHECKED replace, not a racy
  `+dst`).
- **ref grammar** → `git check-ref-format --branch` semantics for `--onto`/`[merge].target_ref`/`--as-branch`
  (reject `refs/…`, `HEAD`, SHAs, `origin/*`, tags, `..`, `.lock`, trailing `/`/`.`, leading `-`).
- **clone-preflight classification** → distinct non-overridable refusals (wrong branch / moved HEAD / dirty),
  keep clone, exit nonzero.
- **re-author identity** → explicit `GIT_AUTHOR_*` + `GIT_COMMITTER_*` env (author==committer==operator) with
  fresh dates, plus the host-commit pins.
- **`source_repo` canon failure** → non-overridable refusal (gone/moved/non-git), keep clone; no override.
- **MINORs** → `merge_run` takes `cfg`; `implement --merge [--onto]` surface; testable source-unchanged
  invariant; `[merge].target_ref` accepts short names only (`refs/heads/main` rejected by the grammar).

## ADR

This increment gets **ADR-0027** (merge hand-off).
