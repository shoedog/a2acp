You are doing a rigorous, adversarial PLAN REVIEW (read-only) of the implementation plan for "E1 —
Worktree-per-Session" for the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). READ-ONLY:
read the plan + the binding spec + the real code; do NOT edit/build/test.

- PLAN: `docs/superpowers/plans/2026-06-23-e1-worktree-per-session.md` (8 TDD tasks T1–T8).
- BINDING SPEC: `docs/superpowers/specs/2026-06-23-e1-worktree-per-session.md` — the `## v2` section (SF-1..6 +
  SR-FIX-1..12).
- The plan claims specific `file:line` anchors + exact type/signature changes. VERIFY each against the real code.

Key code to verify the plan against:
- `crates/bridge-core/src/ports.rs:43-98` — the `AgentBackend` trait (10 methods the decorator must delegate).
- `crates/bridge-core/src/session_cwd.rs` — `SessionCwd::parse`/`as_str`/`is_under` (lexical). `domain.rs:181-192`
  `SessionSpec`.
- `crates/bridge-coordinator/src/session_manager.rs:559-576` (mint: fingerprint from ORIGINAL cwd `:559-563`
  BEFORE `configure_session` `:576`); release `:705-735`; `reap_idle` `:1232-1286` (`release_session` `:1283`).
- `crates/bridge-workflow/src/executor.rs` — the cold-path `configure_session` call that SWALLOWS the error
  (`let _ = ...`, ~`:285`) — SR-FIX-1/T6.
- `bin/a2a-bridge/src/main.rs:482-562` `make_spawn_fn` (the Acp arm `:513-528` is the wrap site); the dead-owner
  liveness sweep `:381`. `bin/a2a-bridge/src/config.rs:115-153` `RegistryConfig` (new `[worktrees]`).
- `bin/a2a-bridge/src/implement.rs` — `run_git` `:264-270`, `assert_dest_outside_worktree` `:441-460`.
- `crates/bridge-core/src/error.rs` — the actual `BridgeError` variants (the plan's T3 uses
  `InvalidStateTransition` for the cwd-immutability reject + `InvalidRequest` for the gate — VERIFY these variants
  exist with those shapes; the SessionManager's real cwd-immutability error is the reference).

{{input}}

GROUND every finding in real `file:line`. Pressure-test the PLAN specifically:

1. **Compile-green per task.** Trace the type ripples. T1 new crate — are the deps + workspace member right? T3's
   `WorktreeBackend` implements `AgentBackend` — does it delegate ALL 10 methods with the EXACT signatures
   (`prompt_observed`, `configure_turn(TurnMeta)`, `reconcile_config`, `capabilities` is SYNC not async,
   `retire`)? Does the plan's `BridgeError::InvalidStateTransition` / `InvalidRequest { field }` match real
   variants in `error.rs`? Does T6's executor edit match the REAL call site + the 3-tuple return
   `(String, bool, Option<UsageSnapshot>)` (Slice-10 changed it)? Flag any task that would NOT compile as written.

2. **Ordering + seam correctness.** Is the bottom-up order right? Does T3 depend on T4's `worktree_path` (the plan
   imports `provider_path` in T3 but defines it in T4 — a forward reference / compile break)? Does the decorator
   actually get every mint/teardown (warm `configure_session`/`release_session` + cold `configure_session`/
   `forget_session`)? Is `capabilities()` (sync) correctly delegated (not `.await`)?

3. **SR-FIX faithfulness.** Each SR-FIX → its task: SR-FIX-2 delegate-then-remove (T3 order); SR-FIX-3 full-trait
   (T3 — any method missed → silent default no-op); SR-FIX-4 idempotent/mismatch (T3 map); SR-FIX-5 self-gate +
   CANONICALIZE (T4 — the plan uses LEXICAL `is_under`, NOT `canonicalize`; is that safe given the source may not
   exist on the host, or is a real canonicalize needed/possible?); SR-FIX-6 root-outside-repo preflight (T5);
   SR-FIX-7 owner/run/hash path + sidecar (T4) + dead-owner sweep (T7); SR-FIX-1 cold configure-error (T6);
   SR-FIX-9 per-request-cwd-only (does T3's `None`/non-repo pass-through actually cover the static-`AcpConfig.cwd`
   bypass, or does static cwd still reach the decorator as `spec.cwd`?). Any SR-FIX under-realized?

4. **The substitute-after-fingerprint invariant.** Confirm the decorator substituting `spec.cwd` inside
   `configure_session` does NOT corrupt the SessionManager fingerprint/immutability guard (the spec CONFIRM). Does
   the warm `continue` path re-call `configure_session` (→ T3 idempotency must hold) or reuse without it?

5. **Lifecycle leaks / cleanup.** Does T7's sweep correctly distinguish dead vs LIVE owners (never reap a live
   worktree)? Is the sidecar metadata sufficient to `git worktree prune` the source after a crash? Does the
   run-workflow end-guard fire on every exit path? Any teardown path (reset/clear generation bump, reconcile-fail,
   release_all on MCP EOF) that drops a session WITHOUT calling release/forget → worktree leak?

6. **Test quality.** Are the TDD tests real failing-tests-first? Does T2's smoke ACTUALLY prove isolation +
   source-clean + cleanup (the keystone)? Does T3 prove delegate-then-remove ORDER + idempotency + mismatch? Does
   T6 prove the node fails WITHOUT prompting? Any test that passes even if the feature is broken?

7. **Missing pieces / wrong anchors / scope.** Any wrong `file:line`. Any SR-FIX with no task. Any step with a
   placeholder, undefined type, or signature mismatch between tasks. Is the per-task granularity right? Anything
   the plan must add or cut (e.g. the sidecar owner-threading seam the plan flags as open)?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. End
with `PLAN VERDICT: ready-to-implement | needs-revision`.
