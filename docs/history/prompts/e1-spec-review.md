You are doing a rigorous, adversarial SPEC REVIEW (read-only) of "E1 — Worktree-per-Session" for the a2a-bridge
(a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). READ-ONLY: read the spec + the real code; do NOT
edit/build/test.

The spec: `docs/superpowers/specs/2026-06-23-e1-worktree-per-session.md`. Binding context (verify every anchor):
- `session_cwd` seam: `SessionCwd` (`crates/bridge-core/src/session_cwd.rs:12-55`, parse + `is_under`),
  `SessionSpec { config, cwd }` (`crates/bridge-core/src/domain.rs:181-192`), the mint at
  `crates/bridge-coordinator/src/session_manager.rs:559-576` (fingerprint + `configure_session`), release at
  `:530-540`.
- The decorator pattern to mirror: `ContainerRwBackend` (`crates/bridge-container/src/lib.rs`), esp. `open_inner`
  cwd-substitution (`:200-297`, `:286-287`), `release_warm` (`:433-445`), `session_cfg` map (`:104-105`).
- B2b git idioms to mirror: `bin/a2a-bridge/src/implement.rs` — `run_git` (`:264-270`), `clone_argv` (`:19-26`),
  `pin_prefix_argv` (`:39-48`), `branch_for` (`:213-215`), `assert_dest_outside_worktree` (`:441-460`); dest at
  `bin/a2a-bridge/src/main.rs:1806`.
- `allowed_cwd_root`: `bin/a2a-bridge/src/config.rs:140` + validation at `crates/bridge-a2a-inbound/src/server.rs:3327-3351`
  and `crates/bridge-coordinator/src/params.rs:263-278`.
- The `AgentBackend` trait (its methods: configure_session/prompt/cancel/forget_session/release_session/...).
- NO existing `git worktree` usage anywhere (greenfield git-shell-out for worktrees).

{{input}}

GROUND every finding in real `file:line`. Pressure-test:
1. **The seam (Q1).** Is a backend DECORATOR (`WorktreeBackend` wrapping the host `AcpBackend`) the right place,
   or should worktree-per-session live in the `SessionManager` so it covers EVERY backend? Does the decorator
   actually see every mint (`configure_session`) and every teardown (`forget_session`/`release_session`) the
   SessionManager + the workflow executor (cold path) drive? Confirm `release_session`/`forget_session` are the
   real teardown calls and that nothing tears a session down WITHOUT calling them (worktree leak).
2. **cwd substitution.** Substituting `spec.cwd` = worktree path at `configure_session` then delegating to inner —
   does that compose with the SessionManager's fingerprint (which captured the ORIGINAL cwd at
   `session_manager.rs:559-563`) and the cwd-immutability guard (reuse-with-different-cwd → InvalidStateTransition)?
   Does the decorator-substituted cwd ever leak back into the fingerprint / a wire surface / a mismatch error?
3. **Lifecycle/keying.** Keyed by `SessionId` — on `continue` is it the SAME SessionId (→ same worktree, continuity)
   and on reset/clear a NEW generation (→ fresh worktree)? Trace the actual SessionId mint (`ctx-{ctx}-g0`) +
   generation bump. Does a cold workflow node (`configure`→`forget` per node) correctly create+remove a worktree
   each node (and is that acceptable cost)?
4. **Gating + safety (SF-3).** Two-sided gate (source under `allowed_cwd_root`; worktree under `worktrees_root`) —
   is it airtight? Can a crafted cwd/worktree path escape (symlink, `..`, a worktree created inside the source)?
   Does `git worktree add` touch the SOURCE in any way that breaks the "source working tree untouched" claim?
5. **Branch strategy (D1/Q2).** `--detach` vs a named branch: does isolation-with-discard-on-remove deliver the
   stated value, or is a named-branch hand-off needed in-slice? What happens to a detached worktree the agent
   committed into (lost commits)?
6. **git edge cases (Q3).** Source repo that is itself a worktree / bare / shallow / has uncommitted changes /
   submodules — does `worktree add --detach <HEAD>` + `worktree remove --force` behave? Any shape that corrupts
   the source or strands a registration?
7. **Concurrency (Q4/D6).** N concurrent `worktree add` to one source — is git's lock + a bounded retry enough, or
   is a per-source serialization mutex required? Any TOCTOU between the gate check and the add?
8. **Cleanup robustness (SF-6).** `worktree remove --force` failure modes (dir deleted, source gone, lock held);
   the drop-guard "never poison"; the boot-sweep (D5). Does a crashed serve leak worktrees + disk?
9. **Scope (Q5) + missing pieces / wrong anchors.** Is host-only + isolation-only the right minimal cut? Any SF
   that won't realize its goal; any wrong `file:line`; anything the spec must add or cut; any spike needed before
   planning (e.g. does `worktree add --detach` then a real agent edit + `worktree remove` actually isolate two
   concurrent sessions on a real repo)?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + `file:line` + a concrete fix. Answer
Q1–Q5 + D1–D6. End with `SPEC VERDICT: ready-to-plan | needs-revision | needs-spike`.
