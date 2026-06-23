# E1 — Worktree-per-Session — SPEC

> Status: DRAFT (architect pass). A Slice-10+ tail item (independent; "pick per value"). Base = `main` `165e7e2`.
> Loop: architect (this) → dual spec-review → plan → dual plan-review → TDD-implement → whole-branch review →
> live-gate → merge.

## Goal
Give each warm session its own **git worktree** off a target repo, so **concurrent write-capable agents don't
clobber each other's working tree**. Reuse the existing `session_cwd` seam: when worktree-mode is on and a
session's cwd is a git repo, materialize a per-session `git worktree` (cheap — shares the source's `.git` object
store, unlike B2b's full clone), substitute its path as the session cwd, and remove it when the session is
released. Opt-in; default off → zero behavior change.

## What ALREADY exists (do NOT rebuild)
- **`session_cwd` (the seam we reuse).** `SessionCwd::parse` (`bridge-core/src/session_cwd.rs:12-42`; absolute +
  lexically-normalized + NUL-free, NO fs access) + component-wise `is_under` (`:51-55`). `SessionSpec { config,
  cwd: Option<SessionCwd> }` (`domain.rs:181-192`). The mint substitutes/consumes cwd at
  `session_manager.rs:574-576` (`backend.configure_session(&backend_session, &SessionSpec { config: eff, cwd })`);
  the fingerprint carries cwd (`:559-563`). **Key invariant (session-cwd-shipped): the host child has NO cwd —
  agents honor the ACP SESSION cwd, not the OS process cwd.** So substituting `spec.cwd` = worktree path makes the
  agent edit IN the worktree.
- **`allowed_cwd_root` gating.** Config field (`config.rs:140`); validated at `server.rs:3327-3351`
  (`session_cwd_from_params`) + `params.rs:263-278` (`validate_cwd`) via `is_under`.
- **B2b clone-quarantine (the pattern we mirror, with worktrees).** `bin/a2a-bridge/src/implement.rs`:
  `run_git(cwd, argv)` = raw `Command::new("git")` with `-C cwd` (`:264-270`); `clone_argv` (`:19-26`);
  `pin_prefix_argv` = `-c safe.directory=<dir> -c core.hooksPath=/dev/null -c commit.gpgsign=false` (`:39-48`);
  `branch_for(task_id)` → `implement/{id}` (`:213-215`); `assert_dest_outside_worktree` (`:441-460`); dest =
  `<allowed_cwd_root>/.a2a-implement/<task-id>/` (`main.rs:1806`). **NO existing `git worktree` usage anywhere.**
- **The decorator pattern (the seam we mirror).** `ContainerRwBackend` (`bridge-container/src/lib.rs`) wraps an
  inner `AgentBackend`: `open_inner` canonicalizes + SUBSTITUTES the cwd (`:286 spec_canon.cwd = rw target`) then
  `inner.configure_session(session, &spec_canon)` (`:287`); `release_warm(session)` reaps per-session (`:433-445`);
  `session_cfg: HashMap<SessionId, SessionSpec>` (`:104-105`). `release_session` is an existing `AgentBackend`
  S0 obligation on ACP + ContainerRw + API.

## The gap E1 closes
Today two warm sessions (or two cold workflow nodes) pointed at the SAME repo cwd share ONE working tree → a
write-capable agent in session A clobbers session B's edits. The B2b `implement` path solves this for ONE write
flow via a full **clone** (expensive; only wired into the `implement` subcommand). There is no general,
cheap, per-session isolation for the warm/serve path. **E1 = worktree-per-session isolation, reusing the cwd seam.**

## Architect decision (the design forks, resolved)
1. **Worktree, NOT clone.** `git worktree add` shares the source's `.git` object store → cheap + fast vs B2b's
   `--no-hardlinks` clone. The source's WORKING tree stays untouched (only `.git/worktrees/<id>` registration is
   added — the same "source untouched" guarantee B2b cares about, at the working-tree level).
2. **A backend DECORATOR (`WorktreeBackend`), NOT SessionManager surgery.** Mirror `ContainerRwBackend`: a new
   `crates/bridge-worktree` wraps the host `AcpBackend`. At `configure_session` it materializes the worktree and
   substitutes `spec.cwd`; at `release_session`/`forget_session` it removes the worktree. The git shell-out lives
   in the new crate (mirroring `implement.rs`'s `run_git`). This keeps `bridge-coordinator`/`SessionManager`
   pure (no git/fs coupling), composes opt-in per-agent like sandbox/container, and aligns the worktree lifecycle
   with the EXISTING configure/forget/release backend calls.
3. **Keyed by `SessionId` (= per-session).** The decorator sees `SessionId` (`ctx-{ctx}-g0`). Concurrent sessions
   (distinct contextIds → distinct SessionIds) get distinct worktrees; a `continue` reuses the same SessionId →
   same worktree (continuity); a reset/clear bumps the generation → new SessionId → fresh worktree. The cold
   workflow path (`configure`→`forget` per node) gets per-node worktrees for free.
4. **HOST path only.** WorktreeBackend wraps a plain host `AcpBackend`. The container compose
   (WorktreeBackend ∘ ContainerRwBackend, mounting the worktree `:rw`) is a tracked DEFERRAL.

So E1's code is concentrated: a new `bridge-worktree` crate (the decorator + the host-git worktree provider) +
a `[worktrees]` config section + the SpawnFn wiring + gating. It reuses the cwd seam, the decorator pattern, and
the B2b git-helper idioms wholesale.

## Design

### SF-1 — `WorktreeBackend` decorator (new `crates/bridge-worktree`)
Implements `AgentBackend` over an inner `Arc<dyn AgentBackend>`:
- `configure_session(session, spec)`: if worktree-mode is enabled AND `spec.cwd` is `Some(repo)` AND `repo` is a
  git repository, call the worktree provider to `git worktree add` a per-session worktree, then delegate
  `inner.configure_session(session, SessionSpec { config: spec.config, cwd: Some(<worktree path as SessionCwd>) })`.
  Store `SessionId → worktree path` in a map. If `spec.cwd` is `None` or not a git repo → **no-op pass-through**
  (delegate the unchanged spec). Worktree creation failure → the configure fails (typed error), no half-state.
- `prompt`/`prompt_observed`/`cancel`/`configure_turn`: delegate to inner unchanged (the inner session already
  carries the worktree cwd).
- `forget_session(session)` and `release_session(session)`: `git worktree remove --force <path>` (best-effort,
  logged), drop the map entry, then delegate to inner. Removal failure is logged, never poisons (drop-guard).

### SF-2 — the host worktree provider (git shell-out, mirrors `implement.rs`)
A `WorktreeProvider` trait (so the decorator is unit-testable with a fake) + a `HostGitWorktree` impl using
`run_git` (raw `Command::new("git") -C <repo>`). Pure argv builders (unit-tested like B2b's):
- `add`: `git -C <repo> worktree add --detach <worktree_path> <base_ref>` (base_ref decided in SF-4).
- `remove`: `git -C <repo> worktree remove --force <worktree_path>`.
- `is_git_repo(path)`: `git -C <path> rev-parse --is-inside-work-tree` (exit 0 + "true").
Use the B2b safe-config pins where relevant (`-c safe.directory=<path>`). The provider canonicalizes paths
(symlink-safe, like ContainerRw) before the containment check.

### SF-3 — worktree path + gating
Worktrees materialize under a gated `worktrees_root` (default `<allowed_cwd_root>/.a2a-worktrees/`, mirroring
B2b's `.a2a-implement/`). The worktree path = `<worktrees_root>/<session-id-hash>/` (hash the SessionId so the
path is filesystem-safe + bounded). Two-sided gate: (a) the SOURCE `repo` (= `spec.cwd`) must be under
`allowed_cwd_root` (already enforced upstream by `session_cwd_from_params`/`validate_cwd`); (b) the worktree path
must be under `worktrees_root` (enforced here, component-wise `is_under`). Reuse `assert_dest_outside_worktree`-
style logic so a worktree is never created inside another repo's tree.

### SF-4 — branch strategy: detached HEAD
`git worktree add --detach <path> <base_ref>` — no named branch, no branch-namespace pollution, simplest cleanup
(`worktree remove` is sufficient; no branch to prune). `base_ref` = the source repo's current `HEAD` (default).
The worktree is for ISOLATION (concurrent non-clobbering), not commit hand-off — edits are discarded on remove. A
B2b-style host-commit-on-release (keep the agent's edits on a branch) is a tracked DEFERRAL (the "persist edits"
follow-on). (Decision flagged for review: detached vs a fresh `a2a-wt/<id>` branch.)

### SF-5 — `[worktrees]` config + opt-in
New top-level `[worktrees]` section in `RegistryConfig` (`config.rs:115-153`, beside `[verify]`/`[implement]`):
`{ enabled: bool (default false), root: Option<String> (default <allowed_cwd_root>/.a2a-worktrees) }`. Opt-in
granularity (per-agent `[agents.worktree]` vs global `[worktrees].enabled` vs per-request
`a2a-bridge.worktree=true`) is decided in review; the spec mandates **opt-in, default off**. Wire the decorator in
the production `SpawnFn` (the sandbox/container wiring site) when enabled.

### SF-6 — cleanup robustness
`worktree remove --force` on release/forget. Handle: worktree dir manually deleted (`git worktree prune` on the
source before remove, or tolerate the remove error); source repo gone (best-effort log); a boot-time sweep of
stale `<worktrees_root>/*` orphaned from a crashed serve (mirror the B2b/container owner-sweep — decided in
review: include a basic prune-on-boot vs defer). A bounded retry on `worktree add` if the source's index/worktree
lock is contended by a concurrent add (mirror B2b's commit-with-retry-on-index-lock).

## Decisions (resolve in dual-review)
- **D1** branch strategy: `--detach` (SF-4) vs a fresh `a2a-wt/<id>` branch (operator can inspect/merge). Detached
  is simpler + pollution-free; named branch enables a future hand-off. Recommend detached for the minimal slice.
- **D2** opt-in granularity: global `[worktrees].enabled` vs per-agent `[agents.worktree]` vs per-request metadata.
  Recommend a global `[worktrees]` block + per-agent enable (mirror `[sandbox]`), per-request override deferred.
- **D3** non-git-repo cwd: no-op pass-through (recommended — worktree-mode only engages for git repos; non-repo
  cwds work as today) vs reject.
- **D4** edits on remove: discarded (detached worktree removed) — acceptable because E1 is ISOLATION, and B2b
  already owns the commit-hand-off write flow. Persist-edits (host-commit-on-release) is a tracked deferral.
- **D5** boot-sweep of orphaned worktrees: include a basic `<worktrees_root>` prune at serve boot vs defer (rely
  on `git worktree prune` + manual cleanup). Recommend a basic prune (orphans leak disk otherwise).
- **D6** concurrency: rely on git's repo lock + a bounded retry on `worktree add` (recommended) vs a serialization
  mutex per source repo.

## Out of scope (tracked deferrals)
- Container compose (WorktreeBackend ∘ ContainerRwBackend, mounting the worktree `:rw`).
- Persist-edits / commit-hand-off on release (B2b already owns the write→commit→merge flow via clone).
- Named-branch-per-worktree + operator merge (depends on D1).
- Worktree-per-NODE policy knobs in the workflow executor (the cold path gets per-node worktrees implicitly via
  configure/forget; no new executor surface).

## Live-gate shape (vs real agents)
With `[worktrees] enabled` + two write-capable host agents (or two contexts on one agent) pointed at the SAME
source repo (under `allowed_cwd_root`):
1. **Isolation:** two CONCURRENT warm sessions (distinct contextIds) each tell the agent to create/modify a
   DIFFERENT file → assert (a) each edit lands in ITS OWN worktree (`git worktree list` on the source shows two
   worktrees; each worktree contains only its own change), (b) the SOURCE working tree is UNTOUCHED (`git status`
   clean), (c) neither session sees the other's file (no clobber).
2. **Continuity:** a `continue` on one context reuses the SAME worktree (the second turn sees the first turn's
   file).
3. **Cleanup:** `release` (or TTL reap) runs `git worktree remove` → `git worktree list` shows the worktree gone,
   no dangling `.git/worktrees/<id>` registration (`git worktree prune` finds nothing to prune).
4. **Gating:** a source repo OUTSIDE `allowed_cwd_root` is rejected (existing gate); a non-git-repo cwd is a clean
   no-op (worktree-mode skipped, session works as today).

## Open questions for the dual spec-review
- Q1: Is the decorator the right seam, or does worktree-per-session belong in the SessionManager (so it covers
  every backend, not just the one the decorator wraps)? Trace whether the decorator sees every mint/release the
  SessionManager drives (warm + cold workflow paths).
- Q2: `--detach` vs named branch (D1) — does isolation-only (discard on remove) deliver the value, or is a
  named-branch hand-off needed in-slice for the value to land?
- Q3: Does `git worktree add` to a source repo that is ITSELF a worktree, a bare repo, or a shallow clone behave?
  Any source-repo shape that breaks the add/remove?
- Q4: Concurrency — N concurrent `worktree add` to one source: is git's lock + a bounded retry sufficient, or is
  a per-source serialization needed (D6)?
- Q5: Scope check — is host-only + isolation-only the right minimal cut, or does the value require the container
  compose or the persist-edits hand-off in-slice?

---

## v2 — dual spec-review folded (codex xhigh needs-revision: 2 BLOCKER + 7 MAJOR + 1 NIT; Opus lens) — BINDING

> Supersedes the draft where it conflicts. The DECORATOR SEAM HOLDS (codex Q1 + Opus confirm) — no
> re-architecture. The folds harden lifecycle/safety, scope to per-request-cwd-only, and move the worktrees root
> OUTSIDE any repo. SPIKE RESOLVED (a real `git worktree add --detach` + two concurrent edits isolate cleanly, the
> source tree stays clean, `remove --force`+`prune` clean up; a worktree created INSIDE the source dirties its
> `git status` → root must be outside). **Ready-to-plan.**

### SR-FIX-1 (codex BLOCKER-1) — the cold executor SWALLOWS configure_session errors
`executor.rs` cold path does `let _ = ...configure_session(...)` (~`:285`), so a worktree-add failure would be
silently ignored and the node would `prompt` in the ORIGINAL/static cwd (isolation silently lost). Fix: the
executor must treat a `configure_session` error as a NODE FAILURE (don't prompt; forget the session; emit the
error marker). The WorktreeBackend's `configure_session` returns a typed error on worktree-add failure and leaves
NO partial worktree (remove-on-failure). (Plan picks: fix the executor — swallowing configure errors is a latent
bug benefiting all backends — vs an explicit warm-only scope; recommend fixing the executor.)

### SR-FIX-2 (codex BLOCKER-2) — teardown ORDER: delegate first, THEN remove the worktree
The draft removed the worktree BEFORE delegating teardown — backwards. `AcpBackend::release_session` CANCELS first
(`crates/bridge-acp/src/acp_backend.rs:2709`); `ContainerRwBackend::release_warm` cancels before reap
(`bridge-container/src/lib.rs:433`). Fix: `release_session`/`forget_session` → delegate to `inner` FIRST (it
cancels/cleans the in-flight session so the agent stops touching the tree), THEN `git worktree remove --force`.

### SR-FIX-3 (codex MAJOR-3) — delegate the FULL `AgentBackend` trait
A decorator that implements only configure/prompt/cancel/forget/release silently DROPS live `reconcile_config`
(SessionManager calls it at `session_manager.rs:475`; trait default at `bridge-core/src/ports.rs:83` would no-op),
`capabilities`, `retire`, `configure_turn`, `prompt_observed`. Fix: delegate EVERY `AgentBackend` method to
`inner`; for `reconcile_config` substitute the already-mapped worktree cwd (not the original).

### SR-FIX-4 (codex MAJOR-4) — idempotent repeated `configure_session` for the same SessionId
Inbound follow-ups reconfigure the SAME session (`bridge-a2a-inbound/src/server.rs:443`); `AcpBackend::configure`
is insert-or-replace (`acp_backend.rs:2605`). Fix: the decorator maps `SessionId → { source, worktree }`. A repeat
configure with the SAME source is IDEMPOTENT (reuse the existing worktree — never create a 2nd); a DIFFERENT source
for a live SessionId is REJECTED (typed mismatch, mirroring the cwd-immutability guard).

### SR-FIX-5 (codex MAJOR-5 + Opus-3) — the decorator SELF-GATES + canonicalizes
Do NOT trust upstream validation: `SessionCwd::is_under` is LEXICAL only (`session_cwd.rs:48`) and `run-workflow
--session-cwd` doesn't check `allowed_cwd_root` (`bin/a2a-bridge/src/main.rs:2690`). Fix: WorktreeBackend
CANONICALIZES source/root/worktree paths (symlink-safe, like ContainerRw `lib.rs:183`) and enforces the
source-under-`allowed_cwd_root` gate ITSELF before any `worktree add`.

### SR-FIX-6 (codex MAJOR-6, SPIKE-CONFIRMED) — worktrees root MUST be OUTSIDE any repo
A worktree created inside the source tree (the draft default `<allowed_cwd_root>/.a2a-worktrees`) DIRTIES the
source's `git status` (spike: git allows it, but it pollutes the source — breaking "source untouched"). Fix: the
worktrees root is a SEPARATE location OUTSIDE `allowed_cwd_root`/any repo. Default = a dedicated state dir
(`$XDG_STATE_HOME/a2a-bridge/worktrees` or `~/.a2a-bridge/worktrees`). Config preflight REJECTS a `[worktrees].root`
that resolves inside a git repo (reuse the `assert_dest_outside_worktree` probe, `implement.rs:441`).

### SR-FIX-7 (codex MAJOR-7 + Opus-1) — owner/lease-aware path + cleanup (NOT a blind `<root>/*` sweep)
Keying by session-hash alone collides across concurrent bridge processes and a blind sweep could delete a LIVE
process's worktree. Fix: worktree path = `<root>/<owner>-<run>-<session-hash>/` (mirror ContainerRw owner/run
identity, `lib.rs:211`) + a sidecar metadata file `{ canonical_source, common_dir, owner, lease }`. The boot-sweep
reaps only DEAD owners (mirror the liveness/lease sweep, `main.rs:381`) — never a live one.

### SR-FIX-8 (codex MAJOR-8) — crash-cleanup needs sidecar metadata + a one-shot end-guard
A gone worktree dir leaves a dangling `.git/worktrees/<id>` registration in the source; a plain root sweep can't
find the source to `git worktree prune` it. Fix: on cleanup, `git worktree remove` then `git worktree prune`
against the recorded canonical source (from the sidecar). Add a SYNCHRONOUS END-GUARD for `run-workflow` (mirror
ContainerRw's `RunEndGuard`) since detached cleanup races process exit. The boot-sweep (REQUIRED, not optional —
was D5) closes the crashed-serve leak.

### SR-FIX-9 (codex MAJOR-9) — scope to PER-REQUEST cwd only (static-cwd bypass documented)
Worktree-mode engages on `spec.cwd = Some` (per-request), but `AcpBackend` falls back to STATIC `AcpConfig.cwd`
(`acp_backend.rs:1651`, from `make_spawn_fn` `main.rs:495`) when no session cwd is stashed → a static-agent-cwd-only
session shares the repo tree, bypassing isolation. Fix (minimal cut): E1 isolates PER-REQUEST-cwd sessions only;
DOCUMENT that a static-`[agents].cwd` agent with no per-request cwd does NOT get a worktree. Threading static cwd
into the decorator is a tracked deferral.

### SR-FIX-10 (codex NIT) — fix anchors
General warm release is `session_manager.rs:705` (`release`/`release_inner`), not only the reconcile-release at
`:530`. Clone dest is `main.rs:1822` (not `:1806`). `is_under` is `session_cwd.rs:48`. Correct before planning.

### SR-FIX-11 (codex Q3) — git-shape policy + tests
Explicit policy + tests: unborn HEAD (empty repo, no commits → `worktree add HEAD` fails → clean typed error, not
a panic); submodules (no auto-init in v1 — document); bare repo → skipped by `is_git_repo` (`--is-inside-work-tree`
= false); source-as-worktree + shallow clone → supported (spike-adjacent). Dirty source changes are NOT copied into
the worktree (worktree starts at the base ref) — note this in the value-prop.

### SR-FIX-12 (codex D2) — hot-reload behavior
The registry reuse key (`AcpConfig` fields) won't naturally wrap/unwrap existing warm backends when
`[worktrees].enabled` toggles at runtime. Fix: DOCUMENT that worktree-enablement changes take effect on the next
fresh spawn (don't silently leave warm backends unwrapped); plan decides whether to include enablement in the
spawn identity.

### CONFIRM (Opus-2 — do NOT let review "fix" this)
Substituting `spec.cwd` = worktree path INSIDE `configure_session` is CORRECT precisely because the SessionManager
builds the fingerprint from the ORIGINAL request cwd at `session_manager.rs:559-563` BEFORE calling
`configure_session`. The worktree path therefore NEVER leaks into the fingerprint or the cwd-immutability guard —
it is a pure internal detail. (Verified: warm release/reap/reconcile all call `release_session`; cold nodes call
`forget_session`; so the in-process teardown is solid — only a CRASHED serve leaks → SR-FIX-7/8 boot-sweep.)

### Updated decisions
- D1 → `--detach` (document: agent commits into a detached worktree are discarded on remove).
- D2 → global `[worktrees]` + per-agent enable; hot-reload per SR-FIX-12.
- D3 → non-git cwd = no-op pass-through (confirmed).
- D4 → discard-on-remove (confirmed; lost-commit note).
- D5 → boot-sweep REQUIRED + owner/lease-aware + sidecar + run-workflow end-guard (SR-FIX-7/8).
- D6 → git repo-lock + bounded retry covering worktree/common-dir lock errors (after unique pathing + canonical
  gates); no per-source mutex.

### Updated live-gate (additions)
Keep the draft's isolation/continuity/cleanup/gating gates, PLUS: (e) the SOURCE `git status` stays CLEAN through
two concurrent sessions (SR-FIX-6); (f) a configured root inside a repo is REJECTED at config preflight (SR-FIX-6);
(g) a worktree-add failure (e.g. unborn HEAD) → the node fails cleanly, no partial worktree, no prompt-in-wrong-cwd
(SR-FIX-1); (h) a simulated crash (kill serve mid-session) leaves an orphan worktree that the boot-sweep reaps on
restart — and a LIVE concurrent process's worktree is NOT reaped (SR-FIX-7).

### Spike status: RESOLVED (git worktree mechanism + isolation + source-clean + root-outside-repo). Ready-to-plan.
