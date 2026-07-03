You are reviewing an IMPLEMENTATION PLAN (not the design) for Slice 3 (Clear / reset) of a2a-bridge, grounded
against the ACTUAL code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git`; do NOT
edit/build/test. Judge whether a codex/sonnet implementor following this plan task-by-task produces correct,
compiling, spec-faithful code. Severity-tag BLOCKER/MAJOR/MINOR. Give concrete fixes (exact code/task edits).

The DESIGN is dual-reviewed + frozen (spec v2, FIX-1..11 binding). Slice 3 adds `clear` = reset a warm
session's CONTEXT to empty (NEW bridge `SessionId` per generation, DIVERGENCE-1) while keeping the PROCESS
warm, with a GENERATION-MONOTONICITY stale-write guard. The plan is below.

{{input}}

READ FOR GROUND TRUTH (cite file:line):
- `docs/superpowers/specs/2026-06-18-slice-3-clear-reset.md` (v2 spec — esp. the "## v2 — dual spec-review
  fixes folded" section: FIX-1 `SessionClear` wire, FIX-2 claim-before-cancel, FIX-3 gen+state guard, FIX-4
  capture-result-then-EXPIRE on fallible configure, FIX-5 release-is-the-drain, FIX-6 fingerprint-superset,
  FIX-7 deferral sites, FIX-8 SessionGeneration::new(get()+1), FIX-9 detached→NotFound, FIX-10 cwd→ConfigInvalid,
  FIX-11 UsageSnapshot::default null).
- `crates/bridge-a2a-inbound/src/session_manager.rs` — `SessionState` (`:18`), `WarmHandle` (`:31`),
  `WarmTurn` (`:52`), `checkout_turn` (mint `:271-309`; the 3 `WarmTurn{..}` returns; the busy-check `~:161`;
  the `Reconciling`/`Expiring` claim discipline `:154-268`), `finish_turn` (`:313`), `record_usage` (`:343`),
  `status()` (`:321`), `release` (`:350`, deferral `:356`), `cancel` (`:369`, deferral `:378`), `reap_idle`
  (`:390`), the `FakeBackend` (`:432`, records `releases()`/`configured()`/`cancels()`), `ManualClock`.
- `crates/bridge-core/src/ids.rs` `SessionGeneration` (`:41`, `new`/`get`); `ports.rs` `release_session`
  (`:55`) + `configure_session` (`:42`, fallible); `domain.rs` `EffectiveConfig`/`SessionSpec`; `error.rs`
  `ConfigInvalid`/`SessionNotFound`/`HandleBusy`/`SessionExpired`/`InvalidRequest`.
- `crates/bridge-acp/src/acp_backend.rs` `AcpBackend::{release_session,cancel}` (`~:1879`/`~:2048`);
  `crates/bridge-container/src/lib.rs` `release_session` (`:553`).
- `crates/bridge-a2a-inbound/src/server.rs` — dispatch match (`:672-684`), `session_release` handler (`:2900`),
  `session_status` (`:2826`), `WarmTurnGuard{sm,ctx}` (`:452`, Drop→`finish_turn(&ctx)`), `warm_local_dispatch`
  (`~:571`), the two usage taps (`spawn_local_producer` `~:1164`, unary `~:2340`), `context_id_arg`.
- `bin/a2a-bridge/src/main.rs` `session_cmd` (`:2724`), the CLI help (`:104`).

REVIEW DIMENSIONS (ground each in code, file:line):
1. **Spec faithfulness** — does each FIX-1..11 map to a task that IMPLEMENTS it? Any scope creep (compact,
   journal, MCP) or gap?
2. **Task ordering / dependency integrity** — does each task COMPILE and pass its own tests at its end? The
   `finish_turn(ctx,gen)`/`record_usage(ctx,gen,snap)` SIGNATURE change (T1) ripples to EVERY call site (the
   producers in server.rs + every direct call in the session_manager/server tests) — does T1 update them all
   so `cargo test --workspace --no-run` passes at T1's end? Does adding `SessionState::Resetting` (T2) break the
   `status()` match (compiler-enforced) and any other exhaustive `SessionState` match — does T2 fix them all?
3. **T1 — the generation guard + threading.** Is `gen == handle.generation && state == Running` the correct
   guard (no-op touches NOTHING on mismatch, FIX-3)? Are all 3 `WarmTurn` construction sites + both producer
   usage taps + the `WarmTurnGuard::Drop` `finish_turn` threaded? Could the change drop a LIVE turn's
   completion (the producer's Drop runs `finish_turn` AFTER the turn — is the handle still Running then?) or
   miss a stale one?
4. **T2 — `reset_session` (highest risk).** Verify against the real concurrency model: is the claim
   (`Idle`→`Resetting` or `Running`+force→`Resetting`, FIX-2 never via Idle/`self.cancel`) atomic under one
   lock? Is capturing `backend` in the claim tuple (not re-locking) right? Is the capture-`configure` result +
   re-acquire + revalidate-exact-claim + commit-or-EXPIRE (FIX-4) correct and free of an early `?` that strands
   `Resetting`? Are the 3 OTHER deferral sites (checkout busy-check, `release`, `cancel`) updated for
   `Resetting` (FIX-7) — or does a `release`/`cancel`/`checkout` during reset reopen the Slice-1 ABA? Is the
   `SessionSpec`-from-fingerprint reconstruction (incl. cwd→`SessionCwd::parse`→`ConfigInvalid`) correct? Does
   `force`'s `backend.cancel(old_id)` + `release_session(old_id)` ordering hold (FIX-5)? Is `usage` zeroed via
   `UsageSnapshot::default()` (FIX-11 null)?
5. **T3 — `SessionClear` wire + CLI.** Is `"SessionClear"` registered in the dispatch match next to the other
   `Session*` (FIX-1, NOT `session/clear`)? Is the handler shaped like `session_release` (auth, sm, ctx, force
   param, `ResetOutcome`→`{cleared,generation}` / `NotFound`→`SessionNotFound` FIX-9)? Does the CLI add `clear`
   + `--force` + the help line, and send `{contextId,force}`?
6. **TDD realizability** — do the tests lean on harness that EXISTS (`FakeBackend.releases()/configured()`,
   `manager()`, `ManualClock`, the server warm-test harness with `with_session_manager`/`RegistryRoute`) or
   need new helpers? Is the `stale_finish_turn_after_reset` test (the GENERATION-MONOTONICITY keystone)
   actually writable + meaningful?
7. **Live-gate provability** — DoD-1 (recall=none after clear), DoD-2 (pgrep process unchanged), DoD-3
   (generation increments, usage nulls), DoD-5 (require-Idle→HandleBusy) provable on real codex via `submit`/
   `session clear`/`session status`? Is DoD-4's precise race correctly unit-gated (live = end-to-end force)?

OUTPUT: findings by severity (task #, file:line, fix); spec-faithfulness verdict; task-ordering verdict;
code-correctness verdict. End: `PLAN VERDICT: ready-to-execute | fix-then-execute | rework`.
