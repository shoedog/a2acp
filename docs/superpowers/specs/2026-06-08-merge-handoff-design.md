# `a2a-bridge merge <id>` — Design Spec (v4, post 2nd spec-review)

**Date:** 2026-06-08
**Status:** Approved (brainstorm). Plan + ADR-0027 to follow.
**Builds on:** ADR-0026 (resume — `resolve_clone`/`load_checkpoint`/`ImplementCheckpoint`), ADR-0019
(B2b-1 — host-commits + the `commit_argv` pin set + bot-identity-pre-merge), ADR-0025 (concurrent runs).
**Reviewed by:** the bridge's own clean-room `design` workflow (codex+claude) AND a dual `spec-review`
(codex *rigor* + claude *soundness*); **v4 folds a SECOND dual `spec-review` (codex+claude, run after the
`usage_update` fix so the claude lens no longer hangs) — see "Spec-review resolutions (round 2)" below.**
**v2 adopts claude's push-based redesign** — it removes the detached
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
   T=<one captured timestamp>   # set BOTH dates to the SAME T so author date == committer date EXACTLY
   GIT_AUTHOR_NAME=<OP> GIT_AUTHOR_EMAIL=<OP> GIT_AUTHOR_DATE=$T \
   GIT_COMMITTER_NAME=<OP> GIT_COMMITTER_EMAIL=<OP> GIT_COMMITTER_DATE=$T \
     git -C <clone> -c safe.directory=<clone> -c core.hooksPath=/dev/null -c commit.gpgsign=false \
         commit-tree <current_commit^{tree}> -p <base_commit> -F -   # message on STDIN → byte-for-byte
   ```
   → a new commit object, **author == committer == operator** with FRESH author/committer dates both set to
   the SAME captured `T` (a clean re-authorship, not a preserved bot date), same tree, parent `base_commit`,
   **without moving the clone's branch** (so a failed push leaves the clone pristine → retry-safe;
   `commit --amend` would move the branch and break the `head_sha == current_commit` preflight on retry).
   *(On the `commit-tree` path only `safe.directory` is load-bearing: `commit-tree` runs NO hooks
   (`core.hooksPath` inert) and signs only on an explicit `-S` (`commit.gpgsign` inert). The pins are kept
   anyway — harmless + uniform with the `commit` path — and the build confirms `gpgsign` behavior on the
   pinned git version. The reused message goes on stdin via `-F -` so a multi-line body/trailers survive.)*
2. **Push it** from the clone to `source_repo`:
   `git -C <clone> push <source_repo> <reauthored>:refs/heads/<target> --force-with-lease=refs/heads/<target>:<base_commit>`.
   - `--force-with-lease=<target>:<base_commit>` **IS the CAS**: the push fast-forwards `target` from
     `base_commit` to `reauthored` ONLY if `target` is still at `base_commit`. If `target` moved → lease
     fails → **refuse** (the v1 "CAS-stale → refuse" decision). Atomic on the receiving side → **no external
     lock needed** (concurrent pushes to one target: one wins, the rest get a stale-lease rejection).
   - **Source-side no-touch guard — the BRIDGE enforces it, not the remote (round-2 BLOCKER fix).** Before
     any Mode-A push the bridge runs a source preflight: confirm `source_repo` is a git repo
     (`git -C <source_repo> rev-parse --git-dir`) and read its checked-out branch
     (`git -C <source_repo> symbolic-ref --short -q HEAD`); if the source is **non-bare and its checked-out
     branch == the resolved target**, **refuse with `CheckedOutTarget` BEFORE pushing**. This is the PRIMARY
     defense for the "without touching the operator's checkout" guarantee — it must NOT rest on the remote's
     `receive.denyCurrentBranch`, which is only a *default*: `updateInstead` would move the operator's
     worktree+HEAD outright, and `warn`/`ignore` would silently desync `refs/heads/<target>` from the
     worktree (the exact ref-vs-worktree corruption the push redesign exists to kill). git's
     `denyCurrentBranch=refuse` stays a **backstop only**; its reject stderr varies by git version, so that
     path is classified conservatively (fallback `Other`). Surfaced as "target is checked out in <source> —
     switch off it or pick another target".
3. **Reap the clone on success**; on any failure keep it + print a recovery command (NOT `rm -rf`).

Nothing is created in `source_repo` except the atomic ref update — so the worktree/ref-leak concerns
(needing `git worktree prune`) **evaporate**.

**Integration-approach comparison (recorded):** push-`commit-tree` beats cherry-pick-in-a-worktree (no
worktree/lock/CAS-ref/temp-ref machinery, force-with-lease is the CAS, the bridge's source preflight +
denyCurrentBranch backstop give checkout safety),
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
| `bin/a2a-bridge/src/implement.rs` | **extract the IDENTITY-FREE git-config pin prefix** from `commit_argv` into a shared helper — just `safe.directory`/`core.hooksPath=/dev/null`/`commit.gpgsign=false`. Identity is NOT shared (round-2 #11): `commit_argv` attaches `BOT` via `-c user.name/email` for `commit`; `reauthor_commit` attaches the OPERATOR via `GIT_AUTHOR_*`/`GIT_COMMITTER_*` env for `commit-tree`. Both call the shared identity-free prefix and attach identity their own way. |

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
/// Validation is a PURE, BEST-EFFORT UX pre-check (round-2 #2) — NOT a claim of full `check-ref-format`
/// parity, and it CANNOT decide repository state (e.g. whether a valid branch name ALSO names an existing
/// TAG — that is the receiver's job, not string grammar). It rejects only what is decidable from the STRING:
/// empty, `HEAD`, raw SHAs (40-hex), any `refs/…` prefix (incl. literal `refs/tags/…`, `refs/remotes/…`), an
/// `origin/…`-style remote prefix, `..`, a component starting with `.` or ending `.lock`, a trailing `/` or
/// `.`, a leading `-`, and any of space/control/`~^:?*[`/`@{`/backslash. **git is the AUTHORITATIVE validator
/// at the push boundary** — an odd name that slips this pre-check fails cleanly there (`Other`). `base_ref`
/// from a checkpoint is run through the same pre-check (it is already a branch name).
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
///   - `CreateBranch` (Mode B, default): non-existence is enforced ATOMICALLY at the receiver via the push
///     refspec asserting an EMPTY/ZERO expected old-value for `refs/heads/{dst}` (a force-with-lease whose
///     lease is "absent") — NOT a separate `branch_exists` read then a plain push (round-2 #3: that races,
///     and a plain push would silently fast-forward an already-existing branch). Exists/raced → `BranchExists`.
///     The exact git invocation for "lease-expects-absent" is verified during the build; the design CONTRACT
///     is: two concurrent Mode-B creates to one name → exactly one creates, the other refuses WITHOUT
///     advancing the branch.
///   - `ReplaceBranch { expect }` (Mode B + `--force`): `--force-with-lease=refs/heads/{dst}:{expect}` where
///     `expect` is the branch's CURRENT tip — a CHECKED replace, never an unconditional `+dst` overwrite (so
///     a concurrent writer is detected → `StaleLease`). Delete races (round-2 #5): if the branch is deleted
///     AFTER `expect` is read, the checked replace fails stale (`StaleLease`) and must NOT recreate it; if
///     deleted BEFORE (so the tip read returns absent), the caller falls back to `CreateBranch`.
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

**`implement --merge` contract (round-2 #8).** Valid on BOTH fresh and `--resume`. It is **mode A only**:
combining `--merge` with `--as-branch` is a parse-time usage error. `--force` is NOT accepted alongside
`--merge` (the sugar runs the merge only when the run ends `Approved`, where the `LoopStopped`+force path
can't arise; the explicit `merge <id> --force` is the escape hatch). Outcome → exit mapping: run reaches
`Approved` → run the merge → the command's exit IS the merge's exit (0 on land; nonzero on any
refuse/preflight, clone KEPT with recovery text); run ends `LoopStopped`/non-terminal → no merge, the
command keeps `implement`'s own nonzero exit; `implement` succeeds but the merge refuses → nonzero, clone
kept, the refusal recovery line printed.

## Control flow

```
merge_cmd(cfg, id, force, onto, as_branch):
  root  = canonicalize(allowed_cwd_root)?; clone = resolve_clone(root, id)?; ck = load_checkpoint(clone)?
  src   = canonicalize(ck.source_repo)?  AND THEN  git -C src rev-parse --git-dir   (round-2 #9:
          canonicalize proves the PATH resolves; rev-parse proves it is still a GIT repo — a dir can
          canonicalize yet be non-git). Either failing ⇒ PreflightFail: "source repo {ck.source_repo}
          gone/moved/not-a-git-repo — keep clone, exit nonzero" (the checkpoint persists the user-supplied
          path; a non-overrideable refusal). NO [merge] override of the stored source.
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
     Merge{target, mode}     => merge_run(cfg, ck, src, &target, mode, force, clone, root)

merge_run(cfg, ck, src, target, mode, force, clone, root):
  op  = operator_from(src, cfg.author.as_ref())?                # fail loud if unset (BOTH halves)
  # round-2 BLOCKER #1 — SOURCE no-touch preflight (PRIMARY guard, not the remote's denyCurrentBranch).
  # Mode A only — Mode B always lands on a fresh/other branch, never the operator's current one:
  if mode == Onto && !is_bare(src) && source_head(src) == Some(target):
       keep clone; return CheckedOutTarget("‹target› checked out in {src} — switch off it / pick --onto")
  msg = ck.original_message.as_deref().unwrap_or(&fallback_subject(ck))   # round-2 #6: reused BYTE-FOR-BYTE
  rt  = reauthor_commit(clone, &ck.current_commit?, &ck.base_commit, msg, &op)?   # commit-tree; clone unmoved
  intent = match mode {
     Onto     => PushIntent::LandOnto { base: &ck.base_commit },          # lease=base; --force NEVER weakens
     AsBranch => match rev_parse(src, target) {                           # read the dst tip ONCE (#5)
        None             => PushIntent::CreateBranch,                     #   absence enforced AT THE PUSH (#3)
        Some(tip) if force => PushIntent::ReplaceBranch { expect: &tip }, #   CHECKED replace (lease=tip)
        Some(_)          => return BranchExists path,                     #   exists && !force
     }
  }
  match push_landing(clone, src, &rt, target, intent):
     Ok(())                => reap_clone(clone, src, root)?; println!("merged {rt} into {target}")  # guarded
     Err(StaleLease)       => keep clone; "‹target› moved off {base_commit} since the clone was made. The
                              clone's base is FIXED, so re-running `merge`/`resume` replays the SAME lease and
                              refuses again. Recovery: manually rebase {clone} onto the moved ‹target› then
                              `merge`, or start a fresh `implement` run. (Auto-replay is deferred.)"   # #4
     Err(CheckedOutTarget) => keep clone; "‹target› is checked out in {src} — switch off it / pick --onto"
     Err(BranchExists)     => keep clone; "branch ‹target› exists — pick a name or pass --force"
     Err(Other(e))         => keep clone; "merge failed: {e}; clone kept at {clone}"

reap_clone(clone, src, root):   # round-2 #7 — guarded delete; NEVER a bare `rm -rf {clone}`
  assert clone.join(".git").exists()  AND  is_under(canonical root, clone)  AND  clone != src
         AND  clone.parent matches the resolve_clone layout dir (`…/.a2a-implement/`)
  → only then remove_dir_all(clone); any assert failing ⇒ KEEP clone + warn (no delete)
```
Exit non-zero on any `Err`/`Refuse`/preflight failure. `ck.current_commit`/`original_message` are `Option` —
the gate refuses `current_commit==None`; `original_message==None` → `fallback_subject(ck)`, which reuses the
B2b-1 first-commit rule EXACTLY: `implement: <first task line, trimmed, ≤120 chars>`, else `implement: changes`.
When `original_message` is `Some`, it is reused **byte-for-byte** (trailing whitespace trimmed) via `-F -`, so a
multi-line body/trailers are preserved.

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
- **`reauthor_commit` dates** — author date == committer date (both set to one captured `T`), and FRESH
  (not the bot commit's date).
- **source HEAD preflight (Mode A, round-2 #1)** — over a temp `source_repo` whose checked-out branch ==
  target: `merge` refuses `CheckedOutTarget` BEFORE any push (source `rev-parse HEAD` + worktree
  byte-identical after); a DIFFERENT checked-out branch lands normally; a **bare** source is not treated as
  checked-out.
- **Mode B atomic absence (round-2 #3)** — two concurrent `CreateBranch` pushes to ONE new name over a temp
  repo: exactly one creates, the other refuses (`BranchExists`) WITHOUT advancing; a `CreateBranch` to a name
  that already exists (even a fast-forwardable one) refuses, never silently FFs.
- **message reuse (round-2 #6)** — `reauthor_commit` reproduces `ck.original_message` byte-for-byte incl. a
  multi-line body/trailer (via `-F -`); `original_message==None` → the exact `implement:` fallback subject.
- **guarded clone reap (round-2 #7)** — `reap_clone` deletes only when clone has `.git` ∧ is under canonical
  root ∧ != source ∧ sits under `.a2a-implement/`; a path failing any guard is KEPT (not deleted).
- **non-git source (round-2 #9)** — a source dir that canonicalizes but is not a git repo refuses
  (`rev-parse --git-dir`), keep clone.
- **Live gate** — operator-run: a real `Approved` run → `merge <id>` lands on the target re-authored, clone
  reaped; a `LoopStopped` run refuses without `--force`; merging onto the **checked-out** branch refuses
  cleanly; `--as-branch` lands a branch; two merges to distinct targets succeed in parallel.

## Build order (smallest shippable slices, docker-free until the live gate)

1. **Pin-prefix extraction** in `implement.rs` (`commit_argv` → shared IDENTITY-FREE prefix; identity stays
   per-caller) + its existing tests stay green.
2. **Pure core** — `MergePlan`/`Mode`/`decide_merge`/`resolve_target` (best-effort pre-check) + the full
   matrix tests.
3. **`reauthor_commit`** (commit-tree, retry-safe, same-`T` dates, `-F -` byte-for-byte message) + temp-repo
   tests.
4. **`push_landing`** (mode A FF + lease; mode B CreateBranch via lease-expects-absent + exists-refusal +
   ReplaceBranch checked) + temp-repo tests incl. the concurrency (two-push) + atomic-absence (two-create)
   tests + source-unchanged invariant. (denyCurrentBranch is the backstop here; the PRIMARY checked-out guard
   is the source preflight in slice 5.)
5. **`merge <id>`** — `merge_cmd` + dispatch + `[merge]` config (fail-loud parse, default tests) +
   `operator_from` fail-loud; the **source preflight** (non-git refusal + Mode-A checked-out refusal) and
   **guarded `reap_clone`**; clone reaped on success / kept on failure (recovery text, no bare `rm -rf`).
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

## Spec-review resolutions (round 2 — codex+claude, v3→v4)

A 2nd dual `spec-review` (run on the bridge's own containerized workflow AFTER the `usage_update` fix, so the
claude soundness lens no longer hangs) returned "not yet ready to plan". All 12 findings folded:

- **BLOCKER #1 — no-touch rested on the remote's default.** The "without touching the operator's checkout"
  guarantee was enforced ONLY by git's default `receive.denyCurrentBranch` on `source_repo`; `updateInstead`
  (moves worktree+HEAD) or `warn`/`ignore` (silent ref↔worktree desync) defeat it. Fix: the BRIDGE now runs a
  **Mode-A source HEAD preflight** (`symbolic-ref --short HEAD` == target on a non-bare repo → refuse
  `CheckedOutTarget` BEFORE pushing); `denyCurrentBranch` is a backstop only.
- **#2 — `resolve_target` "pure ∧ reject tags" was self-contradictory** (tags are repo state, not string
  grammar; "pure check-ref-format parity" overpromised). Reworded to a PURE best-effort UX pre-check (rejects
  only string-decidable forms, incl. literal `refs/tags/…`), with git as the authoritative push-boundary
  validator.
- **#3 — Mode B `CreateBranch` was a TOCTOU** (read-then-push could FF an existing branch). Now absence is
  enforced ATOMICALLY at the receiver via a lease-expects-absent refspec; contract test = two concurrent
  creates, one wins without advancing.
- **#4 — StaleLease recovery text pointed at dead ends** (`re-run merge`/`resume` both replay the SAME fixed
  base lease; `reconcile_head` can't rebase onto a moved tip). Rewritten to the honest recovery (manual rebase
  of the clone, or a fresh `implement`; auto-replay deferred).
- **#5 — Mode B `--force` delete races** classified (delete after tip-read → `StaleLease`, never recreate;
  before → fall back to `CreateBranch`).
- **#6 — reauthored message source** pinned: reuse `ck.original_message` byte-for-byte via `-F -` (multi-line
  body/trailers preserved); explicit `implement:`-prefixed fallback when absent.
- **#7 — success `rm -rf clone` lacked a safety contract.** Added `reap_clone` (delete only when the path is
  the resolved clone: has `.git`, under canonical root, != source, under `.a2a-implement/`).
- **#8 — `implement --merge` surface** pinned: mode-A-only (`--as-branch` is a parse error), `--force`
  rejected, full outcome→exit/print mapping.
- **#9 — non-git `source_repo`** detected by `rev-parse --git-dir` (canonicalize alone can't prove git-ness).
- **#10 — date equality mechanics** — set both `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE` to one captured `T`.
- **#11 — shared prefix is IDENTITY-FREE** (label fixed; each caller attaches identity its own way).
- **#12 — pins largely inert on `commit-tree`** (only `safe.directory` load-bearing; comment + build-time
  `gpgsign` check noted).

## ADR

This increment gets **ADR-0027** (merge hand-off).
