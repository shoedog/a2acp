# E1 — Worktree-per-Session — HANDOFF / Resume Doc

> A Slice-10+ tail item (the user picked E1 from {E1 worktree · E6 retry · E3 batch · E7 task-spec · E8 prompt-lib}).
> **STATUS: architect DONE + PLAN v2 DONE — READY-TO-IMPLEMENT.** Branch `feat/e1-worktree-per-session`
> (base = `main` `165e7e2`). Docs-only so far — NO production code. Read top-to-bottom.

## ⏯️ RESUME POINT: TDD-implement T1 (the plan v2 is locked)
- **Spec = `docs/superpowers/specs/2026-06-23-e1-worktree-per-session.md`** (`## v2`, BINDING; SR-FIX-1..12).
- **Plan = `docs/superpowers/plans/2026-06-23-e1-worktree-per-session.md`** — read the **`## v2`** section
  (BINDING; PR-FIX-1..13 + the "Revised task structure"). It supersedes the draft task bodies above it.
- **Dual plan-review DONE** (codex xhigh `5 BLOCKER + 7 MAJOR + 1 MINOR` + Opus lens, both needs-revision, **no
  re-architecture** — decorator seam holds) → ALL folded into plan v2. Transient: `/tmp/e1-plan-review.out`.
- **Revised task order (plan v2):** T1 crate+provider+argv → **T2 worktree_path + canonicalize/self-gate + full
  sidecar** → **T3 HostGitWorktree + add-failure-cleanup + bounded-retry + isolation smoke** → **T4 WorktreeBackend
  decorator (full-trait delegate + delegate-then-remove + idempotent-RE-DELEGATE + single-flight reserve + retire-
  drains-map + None-bypass test)** → **T5 [worktrees] config + dual-preflight + ALL-call-site make_spawn_fn wiring**
  → T6 cold executor configure-error → **T7 lease-aware boot-sweep + run-workflow end-guard** → T8 unborn-HEAD +
  workspace gate.
- **NEXT: implement task-by-task** (codex-HIGH writes, NO commit / NO git-mutating cmd / `cargo test --workspace
  --all-targets` gate / `_dyld_start` stall → report) → Opus verifies in the clean host env + commits (stage ONLY
  each task's files) → codex-xhigh whole-branch review → live-gate → merge. Reuse a `slice-10-impl`-style codex-HIGH
  config (copy `examples/a2a-bridge.slice-10-impl-codex.toml` + `prompts/slice-10-impl.md` to e1 variants, re-point
  at the E1 plan).
- **The single most critical impl details (plan v2):** (1) `retire()` must DRAIN the worktree map (else leak —
  registry retirement is a real teardown path, `registry.rs:285/327`); (2) `make_spawn_fn` worktree-cfg param
  ripples to ALL call sites (`main.rs:1984/2665/3869/4090` + implement/resume) — `cargo build --workspace` is the
  gate; (3) the sweep is LEASE-aware via `run_identity` (`run_identity.rs:91`), NOT an owner-string compare; (4)
  real lenient CANONICALIZE (mirror `bridge-container/src/lib.rs:713`), not lexical `is_under`.
- Commit history: `81223ae` (spec + spec-review scaffolding) → `1b71455` (spec v2) → `0a0ce0e` (handoff) →
  `28398b0` (plan) → `2451e2f` (handoff update) → `025327b` (plan v2) → plan-review scaffolding (port 8134).

## What E1 is (the architect decision)
Each warm session gets its OWN **git worktree** off a target repo, so **concurrent write-capable agents don't
clobber each other's working tree**. Reuse the `session_cwd` seam: when worktree-mode is on and a session's cwd is
a git repo, materialize a per-session `git worktree --detach` (cheap — shares the source's `.git`, unlike B2b's
full clone), substitute it as the session cwd, remove it at teardown. Opt-in; default off → zero behavior change.
Value = multi-turn-stateful agents (continuity within one worktree) + parallel non-clobbering isolation.

**The seam (both lenses confirmed — no re-architecture):** a `WorktreeBackend` **decorator** (new
`crates/bridge-worktree`) wrapping the host `AcpBackend`, mirroring `ContainerRwBackend`. At `configure_session`
substitute `spec.cwd` = the worktree path (exactly how ContainerRw substitutes the canonical RW cwd at
`lib.rs:286`); delegate-then-`git worktree remove` at `release_session`/`forget_session`. Keyed by `SessionId`
(= per-session; continuity across `continue`, fresh on reset-generation). **Host path only, isolation-only.**

## The real CODE delta (per spec v2)
A new `bridge-worktree` crate (the decorator + a `WorktreeProvider` trait + a `HostGitWorktree` git-shell-out impl
mirroring `implement.rs`'s `run_git`/argv-builders) + a `[worktrees]` config section + SpawnFn wiring + gating.
Reuses the cwd seam, the decorator pattern, and the B2b git idioms wholesale.

**The 12 folded review fixes (spec v2 `## v2`, BINDING):**
- **SR-FIX-1 (BLOCKER):** the cold executor SWALLOWS `configure_session` errors (`let _ =`, ~`executor.rs:285`) →
  a worktree-add failure would silently prompt in the wrong cwd. Fix the executor to fail the node on a configure
  error (recommended — latent bug) OR scope warm-only. **Plan decides.**
- **SR-FIX-2 (BLOCKER):** teardown ORDER = delegate `inner.release_session`/`forget_session` FIRST (it cancels the
  session — `acp_backend.rs:2709`, `container/lib.rs:433`), THEN `git worktree remove`.
- **SR-FIX-3:** delegate the FULL `AgentBackend` trait (`reconcile_config` substituting the mapped worktree cwd,
  `capabilities`, `retire`, `configure_turn`, `prompt_observed`) — defaults would drop live reconcile
  (`session_manager.rs:475`).
- **SR-FIX-4:** idempotent repeated `configure_session` for the same SessionId (`server.rs:443` reconfigures;
  AcpBackend configure = insert-or-replace `acp_backend.rs:2605`) → map `SessionId → {source, worktree}`; same
  source idempotent, different source rejected.
- **SR-FIX-5:** the decorator SELF-GATES + canonicalizes (`is_under` is lexical `session_cwd.rs:48`; `run-workflow
  --session-cwd` doesn't gate `main.rs:2690`) — symlink-safe like ContainerRw `lib.rs:183`.
- **SR-FIX-6 (spike-confirmed):** the worktrees root MUST be OUTSIDE any repo (a worktree inside the source dirties
  its `git status`). Default = a dedicated state dir (`~/.a2a-bridge/worktrees`), NOT under `allowed_cwd_root`.
  Config preflight rejects a root inside a repo (reuse `assert_dest_outside_worktree`, `implement.rs:441`).
- **SR-FIX-7:** owner/lease-aware path `<root>/<owner>-<run>-<session-hash>/` + sidecar metadata
  `{canonical_source, common_dir, owner, lease}`; boot-sweep reaps only DEAD owners (mirror ContainerRw
  `lib.rs:211` + the liveness sweep `main.rs:381`) — never a blind `<root>/*` wipe.
- **SR-FIX-8:** crash-cleanup uses the sidecar to `git worktree prune` the source; a SYNCHRONOUS run-workflow
  END-GUARD (mirror ContainerRw `RunEndGuard`); boot-sweep REQUIRED (closes the crashed-serve leak).
- **SR-FIX-9:** scope to PER-REQUEST cwd only — a static `[agents].cwd` agent (AcpBackend falls back to
  `AcpConfig.cwd` `acp_backend.rs:1651`) does NOT get a worktree in v1 (documented; threading static cwd deferred).
- **SR-FIX-10:** fix anchors (general release `session_manager.rs:705`; clone dest `main.rs:1822`; `is_under` `:48`).
- **SR-FIX-11:** git-shape policy + tests — unborn HEAD (→ clean typed error), submodule (no auto-init v1), bare
  (skipped by `is_git_repo`), source-as-worktree/shallow (supported). Dirty source NOT copied (worktree at base ref).
- **SR-FIX-12:** hot-reload — `[worktrees].enabled` toggling won't wrap/unwrap existing warm backends (registry
  reuse key) → document "takes effect on next fresh spawn."
- **CONFIRM (Opus, do NOT "fix"):** substituting `spec.cwd` INSIDE `configure_session` is correct — the
  SessionManager fingerprints the ORIGINAL cwd at `:559-563` BEFORE configure, so the worktree never leaks into the
  fingerprint/immutability guard. In-process teardown is solid (warm reap/release/reconcile + cold forget all fire);
  only a crashed serve leaks → SR-FIX-7/8 boot-sweep.

## Spike: RESOLVED
`git worktree add --detach <path> HEAD` (path OUTSIDE the source) isolates two concurrent edits (neither sees the
other's file), the SOURCE working tree stays CLEAN (`git status` empty, base file unchanged), and `worktree remove
--force` + `git worktree prune` clean up fully. A worktree created INSIDE the source IS allowed by git but dirties
the source's `git status` → confirms SR-FIX-6 (root outside any repo). No further spike — a T1 worktree-isolation
smoke test (host git, fake/no agent) is the in-plan proof.

## Key seam map (verified file:line — cite in the plan)
- `SessionCwd::parse` `bridge-core/src/session_cwd.rs:12-42`; `is_under` `:48-55`. `SessionSpec{config,cwd}`
  `domain.rs:181-192`.
- Mint + cwd substitution: `bridge-coordinator/src/session_manager.rs:559-576` (fingerprint `:559-563` then
  `configure_session` `:576`). Warm teardown: `release`/`release_inner` `:705-735`; `reap_idle` `:1232-1286`
  (calls `release_session` `:1283`).
- Decorator to mirror: `ContainerRwBackend` `bridge-container/src/lib.rs` — `open_inner` cwd-substitution
  `:200-297` (`:286-287`), `release_warm` `:433-445`, `session_cfg` map `:104-105`, owner identity `:211`.
- B2b git idioms: `bin/a2a-bridge/src/implement.rs` — `run_git` `:264-270`, `clone_argv` `:19-26`, `pin_prefix_argv`
  `:39-48`, `assert_dest_outside_worktree` `:441-460`. Dead-owner liveness sweep `bin/a2a-bridge/src/main.rs:381`;
  SpawnFn `make_spawn_fn` `:495`; `--session-cwd` parse `:2690`.
- `allowed_cwd_root`: `config.rs:140`; top-level `RegistryConfig` `config.rs:115-153` (new `[worktrees]` goes here,
  beside `[verify]`/`[implement]`). `AgentBackend` trait: `bridge-core/src/ports.rs` (reconcile default `:83`).
- NO existing `git worktree` usage anywhere (greenfield git-shell-out).

## Live-gate shape (per spec v2)
`[worktrees] enabled` + two write-capable host agents (or two contexts) on ONE source repo (under
`allowed_cwd_root`): (1) two CONCURRENT warm sessions each edit a DIFFERENT file → each lands in its OWN worktree
(`git worktree list` shows two), neither sees the other's file, SOURCE `git status` CLEAN; (2) `continue` reuses
the same worktree (turn 2 sees turn 1's file); (3) `release`/TTL → `worktree remove`, no dangling registration
(`prune` finds nothing); (4) source outside `allowed_cwd_root` rejected; non-git cwd = clean no-op; (5) source
stays clean through both sessions; (6) a `[worktrees].root` inside a repo rejected at preflight; (7) worktree-add
failure (unborn HEAD) → node fails cleanly, no partial worktree; (8) kill serve mid-session → orphan worktree
reaped by the boot-sweep on restart, a LIVE concurrent process's worktree NOT reaped.

## Proven loop + role matrix + staging (reuse — same as Slice 10)
- **Roles:** codex gpt-5.5 HIGH implements (write, danger-full-access, **NO commit / NO git-mutating cmds**); codex
  gpt-5.5 XHIGH reviews (read-only sandbox); **Opus (controller)** architects/controls/**verifies in the clean host
  env** (codex sandbox stalls on full `--all-targets` runtime → controller re-runs the affected crates)/commits/
  live-gates. codex = default implementor.
- **Scaffolding committed:** spec-review (`examples/a2a-bridge.e1-spec-review-codex.toml` port 8133 +
  `prompts/e1-spec-review.md`). For impl reuse a `slice-10-impl`-style codex-HIGH config
  (`examples/a2a-bridge.slice-10-impl-codex.toml` port 8130 + `prompts/slice-10-impl.md` — copy to e1 variants,
  re-point at the E1 plan). plan-review → next free port (8134); whole-branch → 8135.
- **STAGING DISCIPLINE:** stage ONLY each task's files. The worktree has MANY pre-existing untracked
  `examples/*.toml` / `prompts/*.md` + a pre-existing `M examples/a2a-bridge.slicing-analysis.toml` — NEVER fold
  them.
- **GOTCHAS to carry in:** (1) the controller MUST re-run RUNTIME tests in the host env (codex's sandbox can't —
  the `_dyld_start`/rustc-startup stall blocks them); use `cargo test --workspace --all-targets` (catches stale
  cross-crate counts a `--no-run`/`--bin` gate misses — the Slice-9/10 lesson). (2) A PRE-EXISTING flaky server.rs
  test `warm_streaming_records_usage_without_emitting_usage_frame` (random `messageId` substring) can trip the full
  workspace test once → re-run confirms green; not a regression. (3) The whole-branch dual review keeps catching
  what per-task tests + the happy-path live-gate miss (Slice 10 = 2 MAJOR; cancel-tokens = the ACP-latch cascade).

## After E1 ships
The remaining Slice-10+ tail: E6 retry/resume · E3 batch · E7 typed task-spec · E8 prompt-lib (all independent;
pick per value). Plus E1's tracked deferrals: container compose (WorktreeBackend ∘ ContainerRwBackend `:rw`
worktree mount); persist-edits/commit-hand-off on release; named-branch-per-worktree + operator merge; threading
static agent cwd (SR-FIX-9).
