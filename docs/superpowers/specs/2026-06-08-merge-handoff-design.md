# `a2a-bridge merge <id>` — Design Spec (v6, Mode-A-only, consolidated)

**Date:** 2026-06-08
**Status:** Approved (brainstorm). Plan + ADR-0027 to follow.
**Scope:** **Mode A only** — `--onto <branch>`, fast-forward an accumulating line. Mode B (`--as-branch`,
parallel staging branches) is a **deferred fast-follow** — see "Deferred: Mode B".
**Builds on:** ADR-0026 (resume — `resolve_clone`/`load_checkpoint`/`ImplementCheckpoint`), ADR-0019 (B2b-1 —
host-commits + the `commit_argv` pin set + bot-identity-pre-merge), ADR-0025 (concurrent runs).
**Review provenance:** brainstormed, then **4 dual `spec-review` rounds** (codex *rigor* + claude *soundness*)
run on the bridge's OWN containerized `spec-review` workflow. Both lenses converged on the decomposition being
"sound to plan"; the regression-prone Mode B surface (where all 3 review-found regressions clustered) is
deferred. Condensed round log in the Appendix.

---

## Goal

Automate the manual merge hand-off (`implement::handoff_text`) as **`a2a-bridge merge <id>`** (+ an
`implement --merge` sugar), integrating an `Approved` run's commit into its `source_repo` **without touching
the operator's working checkout** and **safely under concurrent authors**. *(Mode A is a fast-forward off the
run's `base_commit`: if the target advanced past `base_commit` since the clone was made, the merge **refuses**
rather than rewriting — nothing is lost, the operator re-runs off the moved target.)*

## Design: re-author the clone's commit, land it with a lease

The clone (`<allowed_cwd_root>/.a2a-implement/<id>`) is a private, single-author repo whose `branch` holds the
run's work as one effective change over `base_commit` (`current_commit` is its tip, bot-authored). We do NOT
need a worktree to host an index — we re-author in place and push:

1. **Re-author with `git commit-tree`** (NOT `commit --amend`): author AND committer set to the operator via
   explicit env (so the committer can't fall back to ambient git config), reusing the host-commit pins:
   ```
   T=<one captured timestamp>   # set BOTH dates to the SAME T so author date == committer date EXACTLY
   GIT_AUTHOR_NAME=<OP> GIT_AUTHOR_EMAIL=<OP> GIT_AUTHOR_DATE=$T \
   GIT_COMMITTER_NAME=<OP> GIT_COMMITTER_EMAIL=<OP> GIT_COMMITTER_DATE=$T \
     git -C <clone> -c safe.directory=<clone> -c core.hooksPath=/dev/null -c commit.gpgsign=false \
         commit-tree <current_commit>^{tree} -p <base_commit> -F -    # message on STDIN
   ```
   → a new commit object: **author == committer == operator**, fresh dates (both `T`), `current_commit`'s tree,
   parent `base_commit`, **without moving the clone's branch** (a failed push leaves the clone pristine →
   retry-safe; `commit --amend` would move the branch and break the `head_sha == current_commit` preflight on
   retry). *(On `commit-tree` only `safe.directory` is load-bearing — it runs NO hooks and signs only on `-S`;
   the other pins are kept for uniformity; the build confirms `gpgsign` behavior on the pinned git.)*
2. **Push it** into `source_repo` with the lease as the CAS:
   `git -C <clone> push <source_repo> <reauthored>:refs/heads/<target> --force-with-lease=refs/heads/<target>:<base_commit>`
   The lease fast-forwards `target` from `base_commit` to `reauthored` ONLY if `target` is still at
   `base_commit`. If it moved → lease fails → **refuse** (`StaleLease`). Atomic on the receiving side → **no
   external lock** (concurrent pushes to one target: one wins, the rest get a stale-lease rejection).
3. **Reap the clone on success** (guarded); on any failure keep it + print a targeted recovery line (never a
   bare `rm -rf`).

Nothing is created in `source_repo` except the atomic ref update — so worktree/ref-leak concerns evaporate.
*(Approach comparison, recorded: push-`commit-tree` beats cherry-pick-in-a-worktree — no worktree/lock/CAS-ref
machinery, force-with-lease is the CAS — beats `git merge` (no merge bubbles, re-authors), beats
`format-patch`/`am` (3-way fidelity). `git bundle` is a cross-host transport, a deferred seam; `do_clone` is
same-host so a local push suffices.)*

## The gate (`decide_merge`)

Runs before any landing, and **before** the clone HEAD preflight (so an `Option` `current_commit` is resolved
to a hard refusal here, never a misleading "HEAD moved" error later):

- `phase == Approved` **and** `current_commit.is_some()` → `Merge`.
- `phase == LoopStopped` (finished, not approved) → `Refuse` unless `--force`.
- `phase ∈ {Cloned, EditStarted, FirstCommitCreated, InLoop}` (not finished) → **`RefuseHard`** — `--force`
  cannot override ("not finished — `resume` it first").
- `current_commit == None` → **`RefuseHard`** (defensive — an `Approved` run always has `Some`).
- unresolvable `target` → `Refuse`.

On the `Merge` path `current_commit` is guaranteed `Some`, so the clone preflight unwraps it safely.

## Source no-touch guard (best-effort preflight + atomic backstop)

The "without touching the operator's checkout" guarantee has two layers, honestly scoped:

- **Best-effort early refusal (the bridge, UX).** Before the push: confirm `source_repo` is a git repo
  (`git -C <src> rev-parse --git-dir`), then read its checked-out branch
  (`git -C <src> symbolic-ref --short -q HEAD`). If `src` is **non-bare and its checked-out branch == the
  resolved target**, refuse `CheckedOutTarget` BEFORE pushing. Failure-case rules (no silent passes):
  bare `src` → no worktree → proceed; detached HEAD (`symbolic-ref -q` exits 1, no branch) → no branch
  checked out → proceed; branch read OK and `!= target` → proceed.
- **Atomic guarantee (git, the real safety).** git's **default** `receive.denyCurrentBranch=refuse` refuses a
  push to a branch checked out in ANY worktree (main *or* linked) — so the preflight-to-push TOCTOU and
  linked-worktree checkouts are covered by the receive side, not the single `symbolic-ref` read. The preflight
  is the friendly early refusal; **`denyCurrentBranch=refuse` is the guarantee.**
- **Out of scope (documented):** a `source_repo` deliberately configured with a permissive receive policy —
  `receive.denyCurrentBranch=updateInstead` (push-to-deploy, moves the worktree) or `=ignore`/`warn`. If the
  operator chose those semantics, merge does not defend against them; this limitation is stated in the docs.

## Components & file boundaries

| File | Change |
|---|---|
| `bin/a2a-bridge/src/merge.rs` | **NEW** — pure gate (`MergePlan`/`decide_merge`/`resolve_target`) + impure git ops (`operator_from`, `reauthor_commit`, `push_landing`), mirroring `implement_resume.rs` (pure-tested + temp-repo-tested, docker-free). |
| `bin/a2a-bridge/src/main.rs` | `mod merge;`; `merge_cmd` + the `merge` dispatch arm; `run_warm_loop` gains a **typed terminal outcome** so `implement --merge` runs `merge_run` only on `Approved`. |
| `bin/a2a-bridge/src/config.rs` | optional `[merge]` block (`MergeToml`/`MergeConfig`) with a fail-loud `to_config` like `ImplementToml`. |
| `bin/a2a-bridge/src/implement.rs` | **extract the IDENTITY-FREE git-config pin prefix** from `commit_argv` (just `safe.directory`/`core.hooksPath=/dev/null`/`commit.gpgsign=false`). Identity is NOT shared: `commit_argv` attaches `BOT` via `-c user.name/email` for `commit`; `reauthor_commit` attaches the OPERATOR via `GIT_AUTHOR_*`/`GIT_COMMITTER_*` env for `commit-tree`. Reuse `commit_message` for the re-author message (below). |

`merge` runs **no agent**: it must NOT touch the run lease / `RunHandle` / `recover_orphans` / `RunEndGuard`
/ registry / policy / warm session. Its only side effects are the clone-local `commit-tree`, the push, and the
on-success guarded reap. **Concurrency caveat:** because `merge` takes no run lease, it must not run
concurrently with `resume <id>` or a second `merge <id>` on the SAME `<id>`. A *partial* guard exists (the
clone preflight refuses if a concurrent `resume` moved the clone HEAD off `current_commit`); a first-class
per-`<id>` advisory lock shared by `merge`+`resume` is **deferred** — until then the operator serializes
operations on one `<id>`.

## Pure core (unit-tested, git-free)

```rust
pub enum MergePlan {
    Merge { target: String },
    Refuse(String),     // recoverable: LoopStopped w/o --force; unresolvable target
    RefuseHard(String), // non-terminal phase or current_commit==None — --force CANNOT override
}

/// Returns a validated SHORT BRANCH NAME (e.g. `main`, `feature/x`) — NEVER a full ref. `refs/heads/{branch}`
/// is built ONLY at the git boundary (`push_landing`), so `MergePlan.target`, config, and output text all
/// carry the same short-name representation. Precedence: --onto > [merge].target_ref > checkpoint.base_ref;
/// None ⇒ Err. Validation is a PURE, BEST-EFFORT UX pre-check — NOT full `check-ref-format` parity, and it
/// canNOT decide repo state (a valid name may also be a tag — git decides that at the push boundary). Rejects
/// only string-decidable forms: empty, `HEAD`, raw SHAs (40-hex), any `refs/…` prefix (incl. `refs/tags/…`,
/// `refs/remotes/…`), an `origin/…`-style prefix, `..`, a component starting `.` or ending `.lock`, a trailing
/// `/` or `.`, a leading `-`, and any of space/control/`~^:?*[`/`@{`/backslash. git is authoritative at push.
pub fn resolve_target(cli_onto: Option<&str>, cfg: Option<&str>, base_ref: Option<&str>) -> Result<String, String>;

/// Mode-independent. (Mode B's fast-follow reuses it unchanged.)
pub fn decide_merge(phase: ImplementPhase, has_commit: bool, force: bool, target: &Result<String, String>) -> MergePlan;
```

## Impure ops (temp-repo tested, docker-free)

```rust
pub struct OperatorIdent { name: String, email: String }
/// source_repo git config user.name+user.email (or [merge] override). FAIL LOUD if EITHER half is missing.
/// A config override must supply BOTH halves or it's a parse error.
pub fn operator_from(repo: &Path, cfg_override: Option<&OperatorIdent>) -> Result<OperatorIdent, String>;

/// commit-tree current_commit's tree over base_commit as the operator → the re-authored sha. Same `T` for
/// both dates; message via `-F -`. Does NOT move the clone's branch (retry-safe). Reuses the identity-free pin
/// prefix. The CALLER first runs the clone shape/ancestry preflight (below).
pub fn reauthor_commit(clone: &Path, current_commit: &str, base_commit: &str, msg: &str, op: &OperatorIdent) -> Result<String, String>;

pub enum PushError { StaleLease, CheckedOutTarget, Other(String) }
/// Push `reauthored` into source_repo as `refs/heads/{target}` (built here from the short name) with
/// `--force-with-lease=refs/heads/{target}:{base_commit}` — FF iff the target is still at base_commit.
/// CLASSIFICATION (no stderr parsing): on a non-zero push, read `refs/heads/{target}` in source_repo —
///   • target != reauthored AND != base_commit (it moved) → `StaleLease`.
///   • the rare denyCurrentBranch backstop race (target == source HEAD, slipped the preflight) → `Other`
///     (conservative; its stderr varies by git version). `CheckedOutTarget` is produced by the PREFLIGHT,
///     deterministically, never by parsing push stderr.
pub fn push_landing(clone: &Path, source_repo: &Path, reauthored: &str, target: &str, base_commit: &str) -> Result<(), PushError>;
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
**`MergeToml::to_config` validation:** `target_ref`, if present, is a non-empty string passing `resolve_target`'s
pre-check (empty/blank → parse error). Identity override is **both-or-neither**: `author_name` XOR
`author_email` → parse error; both absent → `author = None`. **No env-var expansion** (merge takes literal
strings — unlike `[delegation]`, the only block that expands `${…}`). Unknown keys are IGNORED, matching the
rest of `RegistryConfig` (no `deny_unknown_fields`). (No `lock_wait_secs` — there is no lock.)

## Command surface

```
a2a-bridge merge <id> [--config <path>] [--onto <branch>] [--force]
a2a-bridge implement <task> --repo <path> … [--merge [--onto <branch>]]   # Approved-only sugar
a2a-bridge implement --resume <id> …       [--merge [--onto <branch>]]
```
`--onto` target selection: `--onto` if present, else `[merge].target_ref`, else `checkpoint.base_ref`;
`base_ref == None` (HEAD-based run) with no config target → fail loud.

**`implement --merge` contract.** Valid on BOTH fresh and `--resume`. `--force` is NOT accepted alongside
`--merge` (the sugar merges only when the run ends `Approved`, where the `LoopStopped`+force path can't arise;
explicit `merge <id> --force` is the escape hatch). The mapping is enabled by the new typed terminal outcome
from `run_warm_loop` (TODAY it returns `()` and prints the hand-off internally, both callers `Ok(())` — the
spec ADDS the typed return). **Plain `implement` without `--merge` keeps its current exit behavior unchanged**
— the typed outcome only drives the `--merge` path.

**Exit codes** (so a CI caller can branch on `$?`):

| code | meaning |
|---|---|
| `0` | merged (or, without `--merge`, implement succeeded per existing semantics) |
| `1` | usage / config / preflight error (bad args, schema mismatch, source gone, clone preflight, operator unset) |
| `2` | under `--merge`: the run did NOT reach `Approved` (LoopStopped/non-terminal) — *re-run/resume the agent* |
| `3` | under `--merge`: `Approved` but the merge could not land (`StaleLease`/`CheckedOutTarget`) — *retry the land off a fresh target* |

## Control flow

```
merge_cmd(cfg, id, onto, force):
  root = canonicalize(allowed_cwd_root)?; clone = resolve_clone(root, id)?; ck = load_checkpoint(clone)?
  # SCHEMA GATE (NEW — load_checkpoint only deserializes; resume does not gate this either; merge is the first
  # consumer). Non-overridable:
  if ck.schema_version != SCHEMA_VERSION:
       eprintln "checkpoint schema {ck.schema_version} unsupported (merge expects {SCHEMA_VERSION}) — rebuild
                 with a current run"; exit 1
  src = canonicalize(ck.source_repo)?  AND THEN  git -C src rev-parse --git-dir   (canonicalize proves the
        PATH resolves; rev-parse proves it is still a GIT repo — a dir can canonicalize yet be non-git). Either
        failing ⇒ "source repo {ck.source_repo} gone/moved/not-a-git-repo — keep clone, exit 1". NO override.
  target = resolve_target(onto, cfg.target_ref, ck.base_ref.as_deref())
  match decide_merge(ck.phase, ck.current_commit.is_some(), force, &target):    # gate FIRST (resolves None)
     Refuse(m)    => eprintln(m); exit 1      # (under --merge: phase!=Approved ⇒ exit 2)
     RefuseHard(m)=> eprintln(m); exit 1      # (under --merge: non-terminal/None ⇒ exit 2)
     Merge{target}=> # current_commit is now guaranteed Some
       # CLONE PREFLIGHT (cheap, impure) — each a NON-overridable refusal (force ignored), KEEP clone, exit 1,
       # DISTINCT recovery text; guards retry-safety so it runs before any push:
       #   current_branch(clone) != ck.branch                       → "clone on wrong branch — inspect {clone}"
       #   head_sha(clone)       != ck.current_commit               → "clone HEAD moved off the checkpoint"
       #   is_worktree_dirty(clone)                                 → "clone worktree dirty — inspect {clone}"
       # CLONE SHAPE / ANCESTRY (guards the commit-tree graft against a corrupted/unexpected clone — the bridge
       # owns the dir, this is integrity not adversarial defense):
       #   git -C clone cat-file -e base_commit^{commit}  AND  current_commit^{commit}   (objects exist)
       #   git -C clone merge-base --is-ancestor base_commit current_commit              (base ⊑ current)
       #     any failing → "clone history unexpected (base not an ancestor of the run commit) — inspect {clone}"
       merge_run(cfg, ck, src, &target, clone, root)

merge_run(cfg, ck, src, target, clone, root):
  op  = operator_from(src, cfg.author.as_ref())?                     # fail loud if unset (BOTH halves)
  # SOURCE no-touch preflight — keyed off resolved-target vs the source's checked-out branch (best-effort;
  # denyCurrentBranch=refuse is the atomic backstop):
  if !is_bare(src) && source_head(src) == Some(target):
       keep clone; eprintln "‹target› checked out in {src} — switch off it / pick another"; exit 3
  msg = commit_message(ck.original_message.clone(), &ck.task_brief).0  # ONE call: reuse-or-fallback (no
                                                                       # fabricated helper; Some→trimmed verbatim,
                                                                       # None/empty→`implement: <brief ≤120>`)
  rt  = reauthor_commit(clone, &ck.current_commit?, &ck.base_commit, &msg, &op)?      # commit-tree; clone unmoved
  match push_landing(clone, src, &rt, target, &ck.base_commit):
     Ok(())                => reap_clone(clone, src, root)?; println!("merged {rt:.12} into {target}")  # exit 0
     Err(StaleLease)       => keep clone; eprintln "‹target› moved off {base_commit} since the clone was made.
                              The clone's base is FIXED and the clone preflight refuses a moved HEAD, so re-
                              running `merge`/`resume` can't land it. Recovery: start a FRESH `implement` run
                              off the moved ‹target›. (A checkpoint-updating replay is deferred.)"; exit 3
     Err(CheckedOutTarget) => keep clone; eprintln "‹target› is checked out in {src} — switch off it"; exit 3
     Err(Other(e))         => keep clone; eprintln "merge failed: {e}; clone kept at {clone}"; exit 3

reap_clone(clone, src, root):   # guarded delete; NEVER a bare `rm -rf {clone}`
  croot = canonical(root); cclone = canonical(clone)?; csrc = canonical(src)?
  assert cclone == croot.join(".a2a-implement").join(id)  AND  cclone.join(".git").is_dir()
         AND  is_under(croot, cclone)  AND  cclone != csrc     # symlinks resolved by canonicalize on both sides
  → only then remove_dir_all(cclone); any assert failing ⇒ KEEP clone + warn (no delete)
```

## Testing strategy

Pure core unit-tested; git ops over temp repos (docker-free); `merge_cmd` + `--merge` sugar live-gated.
- **`decide_merge`** — full phase × `has_commit` × force matrix; keystones: non-terminal+force → `RefuseHard`;
  `current_commit==None` → `RefuseHard` (and refused BEFORE the clone HEAD comparison); target Err → `Refuse`.
- **`resolve_target`** — precedence; best-effort rejects (HEAD/SHA/`refs/…`/`origin/…`/trailing `.lock`/leading `-`); None→Err.
- **`reauthor_commit`** — author==committer==operator (NOT bot); `current_commit`'s tree; parent==`base_commit`;
  the clone's branch is **unmoved** (retry-safe); author date == committer date (both the captured `T`), FRESH.
- **message reuse** — `commit_message` reproduces `ck.original_message` verbatim incl. a multi-line body/trailer
  (already trimmed at capture, no second trim); `None`/empty → the `implement:` fallback subject.
- **clone shape/ancestry** — a clone whose `base_commit` is NOT an ancestor of `current_commit`, or with a
  missing object, refuses (force ignored), keeps the clone.
- **`push_landing`** over temp repos — FF when `target == base_commit`; **StaleLease** when the target moved
  (asserted by OBSERVABLE post-failure ref state — did `refs/heads/<target>` move? — not by stderr text).
- **source HEAD preflight** — non-bare `src` whose checked-out branch == target → `CheckedOutTarget` BEFORE any
  push (`src` `rev-parse HEAD` + `status --porcelain` byte-identical after); a DIFFERENT checked-out branch
  lands; a **bare** src and a **detached-HEAD** src both proceed.
- **concurrency** — two `push_landing` to ONE target over a temp repo: exactly one succeeds, the other
  StaleLease (no lock; force-with-lease is the CAS).
- **source-unchanged invariant** — non-bare temp `src` only: capture `git rev-parse HEAD` +
  `git status --porcelain=v1 --untracked-files=all` before/after; assert byte-identical; ONLY
  `refs/heads/<target>` may move.
- **`operator_from`** — sources repo git config; fail-loud when unset; `[merge]` override (both halves) wins;
  half-override → error.
- **`MergeToml::to_config`** — empty `target_ref` → error; half identity override → error; both absent →
  `author=None`; unknown keys ignored; no env expansion.
- **clone preflight + schema + non-git source** — wrong-branch / moved-HEAD / dirty each refuse (force ignored,
  exit 1, distinct messages); a schema-version mismatch refuses (exit 1); a gone/non-git `source_repo` refuses.
- **guarded reap** — `reap_clone` deletes only when `cclone == <root>/.a2a-implement/<id>` (post-canonicalize) ∧
  has `.git` ∧ under root ∧ != source; a path failing any guard is KEPT.
- **exit codes** — landed → 0; preflight/schema/usage → 1; `--merge` non-Approved → 2; `--merge` Approved-but-
  unlanded → 3.
- **CLI/output contract** — success stdout includes `merged <sha> into <target>`; every keep-clone failure
  emits a stderr cause line + the clone path. Tests assert these FIELDS, not full wording.
- **Live gate** — operator-run: a real `Approved` run → `merge <id>` lands on the target re-authored, clone
  reaped (exit 0); a `LoopStopped` run refuses without `--force`; merging onto the **checked-out** branch
  refuses cleanly (exit 3); a moved target → StaleLease recovery line (exit 3); `implement --merge` lands.

## Build order (smallest shippable slices, docker-free until the live gate)

1. **Pin-prefix extraction** in `implement.rs` (`commit_argv` → shared IDENTITY-FREE prefix; identity stays
   per-caller) + its existing tests stay green.
2. **Pure core** — `MergePlan`/`decide_merge`/`resolve_target` + the full matrix tests.
3. **`reauthor_commit`** (commit-tree, retry-safe, same-`T` dates, `-F -`, reuses `commit_message`) +
   clone shape/ancestry preflight + temp-repo tests.
4. **`push_landing`** (FF + lease; observable-state classification) + temp-repo tests incl. the concurrency
   (two-push) test + the source-unchanged invariant.
5. **`merge <id>`** — `merge_cmd` + dispatch + `[merge]` `MergeToml::to_config` (fail-loud + validation tests) +
   `operator_from` fail-loud + the schema gate + the source no-touch preflight + guarded `reap_clone` + the exit
   codes; clone reaped on success / kept on failure (recovery text, no bare `rm -rf`).
6. **`run_warm_loop` typed outcome + `implement --merge`** sugar (Approved-only).

## Risks

- **Operator identity unset on headless hosts** — fail-loud + `[merge]` override + an unset test.
- **`base_ref == None`** — `resolve_target` errs explicitly; `--onto`/`[merge].target_ref` make it turnkey.
- **Re-author idempotency** — `commit-tree` (not amend) leaves the clone unmoved, so a failed push is cleanly
  retryable; the `head_sha == current_commit` preflight still holds on retry.
- **Discarded work under concurrency** — under genuine concurrency, target-moved (`StaleLease`) is the *common*
  case, and each collision discards an `Approved` run's agent/container cost. The architecture keeps the replay
  seam open (clone retained + `commit-tree` pristine) but does NOT auto-replay yet → a bounded retry-replay
  (cherry-pick onto the moved tip in the clone, then push) is the motivated follow-up. Do not read Mode A as
  production-resilient under heavy concurrency.
- **Permissive `receive.denyCurrentBranch`** — `updateInstead`/`ignore`/`warn` on `source_repo` are out of
  scope (see the no-touch guard); documented, not defended.

## Deferred: Mode B (`--as-branch`) — fast-follow

A separate slice (its own review) adds **Mode B**: push the re-authored commit to a **new**
`refs/heads/<name>` (default `implement/<task_id>`) for *parallel* tasks in one slice. It re-introduces a
`PushIntent { LandOnto, CreateBranch, ReplaceBranch }` enum (the gate stays mode-independent), where
`CreateBranch` enforces non-existence ATOMICALLY at the push (lease-expects-absent — the exact git refspec
verified during that slice's build, with a two-create concurrency test) and `--force` = `ReplaceBranch{expect=tip}`
(a checked replace, with delete-race classification). It is deferred because all three regressions the review
rounds found lived in this surface; Mode A ships first and clean.

## ADR

This increment gets **ADR-0027** (merge hand-off — Mode A).

## Appendix: revision history (condensed)

- **v1** — worktree + cherry-pick + `cas_advance`/temp-ref + per-target lock.
- **v2** — claude's push-based redesign (`commit-tree` + `--force-with-lease`); removed the worktree /
  `WorktreeGuard` / `.a2a-merge/` / CAS-ref / temp-ref / per-target lock; fixed the silent checkout-corruption
  BLOCKER.
- **v3** — folded codex rigor round 1 (short branch-name invariant; ref grammar; classified clone-preflight
  refusals; explicit `GIT_AUTHOR_*`/`GIT_COMMITTER_*` re-author).
- **Round 2 (→v4)** — BLOCKER: the no-touch guarantee can't rest on the *default* `denyCurrentBranch` → added a
  source HEAD preflight; +11 precision folds. (Run after the `usage_update` SDK fix, so the claude lens stopped
  hanging.)
- **Round 3 (→v5)** — caught a REGRESSION: the v4 preflight was mode-gated (`mode == Onto`), so Mode B
  `--as-branch <live-branch> --force` slipped onto the operator's checkout → ungated the preflight; + StaleLease
  dead-end recovery, the Mode B "No lease/CAS" self-contradiction, the `--merge` exit mapping, etc.
- **Round 4 (→v6)** — both lenses "sound to plan". **Deferred Mode B** (all 3 regressions clustered there) and
  **consolidated** this doc. Folded: clone shape/ancestry preflight; a REAL `schema_version` gate (v5 wrongly
  claimed `load_checkpoint` already validates it — it doesn't); `current_commit==None` ordering; the preflight
  reframed best-effort + `denyCurrentBranch=refuse` atomic + `updateInstead` out-of-scope; `PushError`
  observable-state classification; `reap_clone` canonicalization; distinct exit codes (2 vs 3); the
  discarded-work Risk; and fixed a fabricated `fallback_subject` → the real `commit_message(raw, task)`.
