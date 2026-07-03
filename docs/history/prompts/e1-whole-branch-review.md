You are doing a rigorous, adversarial WHOLE-BRANCH REVIEW (read-only) of the FULLY-IMPLEMENTED "E1 —
Worktree-per-Session" feature for the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator).
READ-ONLY: read the diff + the binding spec/plan + the real code; do NOT edit/build/test.

E1 gives each warm session its OWN `git worktree` off a target repo so concurrent write-capable agents don't
clobber each other's working tree — via a `WorktreeBackend` DECORATOR (new `crates/bridge-worktree`) wrapping the
host `AcpBackend`, reusing the `session_cwd` seam. Opt-in via `[worktrees]`; host-only; isolation-only;
per-request-cwd only.

- BINDING SPEC: `docs/superpowers/specs/2026-06-23-e1-worktree-per-session.md` (the `## v2` section, SF-1..6 +
  SR-FIX-1..12).
- BINDING PLAN: `docs/superpowers/plans/2026-06-23-e1-worktree-per-session.md` (the `## v2` section, PR-FIX-1..13).
- The branch is `feat/e1-worktree-per-session`; the 8 implementation commits are T1..T8 (the `feat(worktree)`/
  `feat(workflow)` commits). The diff vs `main` is below.

The IMPLEMENTATION surface (read every file):
- `crates/bridge-worktree/src/provider.rs` — `WorktreeProvider` trait (`add -> Result<String>` = common_dir,
  `remove`, `is_git_repo`) + pure git argv builders.
- `crates/bridge-worktree/src/provider_path.rs` — `WorktreeConfig`, `ResolvedWorktree`, `resolve_worktree`
  (canonicalize + self-gate), `WorktreeSidecar` + `sidecar_path`/`write_sidecar`/`read_sidecar`.
- `crates/bridge-worktree/src/host_git.rs` — `HostGitWorktree` (real git, bounded lock-retry, add-failure cleanup,
  common-dir capture).
- `crates/bridge-worktree/src/backend.rs` — `WorktreeBackend` decorator (the core: configure_session substitute +
  single-flight + idempotent re-delegate + delegate-then-remove + retire-drains-map; all 10 `AgentBackend` methods).
- `crates/bridge-worktree/src/sweep.rs` — `sweep_orphans` (lease-aware dead-owner reap) + `WorktreeRunEndGuard`.
- `bin/a2a-bridge/src/config.rs` — `[worktrees]` parse + `preflight_worktrees_root` (dual: outside any repo AND
  outside `allowed_cwd_root`).
- `bin/a2a-bridge/src/main.rs` — `resolve_worktree_runtime_cfg`, `make_spawn_fn` worktree param + Acp-arm wrap,
  boot-sweep + run-workflow end-guard wiring.
- `crates/bridge-workflow/src/executor.rs` — cold path now FAILS the node on `configure_session` error (SR-FIX-1).

Reference code to verify against:
- `AgentBackend` trait (`crates/bridge-core/src/ports.rs:43-98`, 10 methods, `capabilities` SYNC).
- `session_cwd` (`crates/bridge-core/src/session_cwd.rs`: `parse`, `is_under` lexical). `SessionSpec`
  (`domain.rs:181-192`). The mint: `crates/bridge-coordinator/src/session_manager.rs:559-576` (fingerprint from
  ORIGINAL cwd `:559-563` BEFORE `configure_session` `:576`); release `:705-735`; `reap_idle` `:1232-1286`.
- `run_identity::classify` (`crates/bridge-core/src/run_identity.rs:91-112`) + `liveness::LeaseProbe`. The container
  decorator to compare against: `crates/bridge-container/src/lib.rs` (`canonicalize_lenient` `:713`, `release_warm`,
  the `impl AgentBackend` `:449-607`). `recover_orphans`/`RunEndGuard` (`bin/a2a-bridge/src/main.rs:381-448`).

{{input}}

GROUND every finding in a real `file:line`. Pressure-test the WHOLE feature for CORRECTNESS + SAFETY + LEAKS:

1. **The decorator lifecycle (the core).** Does substituting `spec.cwd` inside `configure_session` corrupt the
   SessionManager fingerprint/immutability guard (it shouldn't — fingerprint is captured BEFORE configure)? Trace
   it. Is the single-flight `Reserving` claim correct under real concurrency (could two adds still happen; could the
   `yield_now` spin live-lock or strand a `Reserving` entry forever on an error path)? Does same-source re-configure
   RE-DELEGATE (not no-op)? Is the different-source reject correct (`ConfigMismatch{field:"cwd"}`)? Is the
   teardown order ALWAYS delegate-inner-FIRST then remove (release/forget/retire)?

2. **Leaks.** Walk EVERY teardown path: warm `release_session`/`reap_idle`/reset-clear-generation/reconcile-fail/
   `retire` (registry `registry.rs:285/327`) + cold `forget_session` + MCP `release_all`/EOF. Does any drop a
   session WITHOUT calling release/forget on the decorator → orphan worktree? Is `retire()` draining the map (PR-FIX-5)?
   Does a `Reserving` entry at teardown leak (and is the boot-sweep the correct backstop)? Does the crash path leak
   only what the boot-sweep reaps?

3. **Gating / path safety.** Can a crafted cwd/worktree path escape the gate (symlink, `..`, a worktree created
   INSIDE the source)? Is the canonicalization real (not lexical) and applied BEFORE any git op? Is the dual root
   preflight airtight (root inside a repo OR under `allowed_cwd_root` → rejected) across serve + run-workflow +
   implement? Is `allowed_cwd_root` REQUIRED when worktrees enabled?

4. **The sweep (never reap a LIVE worktree).** Does `sweep_orphans` correctly spare a live (lease-held) owner AND
   another host via `classify`? Could it reap THIS run's worktrees at boot? Is the `WorktreeRunEndGuard` `run_id`
   filter correct (never touches a concurrent run)? Is the sidecar sufficient to clean up after a crash (common_dir/
   prune)? Any TOCTOU between scan and remove?

5. **The wiring.** Is the new `make_spawn_fn` worktree param threaded to ALL call sites (no path silently bypassing
   the decorator)? Does the Acp-arm wrap derive identity correctly from the `RunHandle`? Is the owner/run/hash path
   collision-free per (agent, process, session)? Does the cold executor fix (SR-FIX-1) correctly fail the node
   without prompting, and does it not regress the existing cold tests?

6. **git edge cases + concurrency.** unborn HEAD / bare / shallow / source-as-worktree / submodule / dirty source —
   any shape that corrupts the source or strands a registration? N concurrent `worktree add` to one source — is the
   bounded lock-retry + single-flight enough? Does `worktree add --detach` leave the SOURCE working tree untouched?

7. **Anything missed / wrong / over-built.** Any SR-FIX/PR-FIX under-realized vs the binding spec/plan. Any wrong
   `file:line`. Any dead code, any test that passes even if the feature is broken, any scope creep beyond the
   documented deferrals (container compose, persist-edits, named-branch, static-cwd threading).

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. End
with `REVIEW VERDICT: ship | fix-then-ship | needs-rework`.
