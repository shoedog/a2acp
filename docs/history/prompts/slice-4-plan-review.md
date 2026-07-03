You are reviewing an IMPLEMENTATION PLAN (not the design) for Slice 4 (Compact) of a2a-bridge, grounded against
the ACTUAL code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git`; do NOT edit/build/test.
Judge whether a codex implementor following this plan task-by-task produces correct, COMPILING, spec-faithful
code that passes each task's tests AND `cargo test --workspace --no-run` at each task boundary. Severity-tag
BLOCKER / MAJOR / MINOR. Give concrete fixes (exact task/code edits, file:line).

The DESIGN is dual-reviewed + frozen: spec `docs/superpowers/specs/2026-06-18-slice-4-compact.md` v2, **FIX-1..14
BINDING** (esp. FIX-1 bad-summary EXPIRES the handle — never restore-Idle, because the summarize is a real
`backend.prompt` on the live session that irreversibly mutates the old context). Slice 4 = `compact` = summarize
gen N → reset to N+1 → seed the summary as the next turn's first part; require-Idle, no force; composition over
the SHIPPED `reset_session`. The plan is below.

{{input}}

READ FOR GROUND TRUTH (cite file:line):
- The v2 spec (esp. the "## v2 — dual spec-review fixes folded" FIX-1..14 list).
- `crates/bridge-a2a-inbound/src/session_manager.rs` — `SessionState`/`is_claimed` (`:19`/`:32`), `WarmHandle`
  (`:39`), `WarmTurn` (`:60`), `checkout_turn` (the 3 `WarmTurn{..}` sites `:198`/`:268`/`:310`; the clean-diff
  success `:193-205`; the post-reconcile clean success `:261-275`; the non-clean EXPIRE tail `:276-292`; the
  mint `:295-335`), `finish_turn` (`:341`), `record_usage` (`:375`), `status` (`:352`), `release`/`cancel`
  (`:391`/`:411`), `reset_session` (`:434`, the claim block `:440-471` + the commit/EXPIRE tail `:475-519`),
  `reap_idle` (`:523`), the struct + `new`/`new_with_clock`/`with_warn_fraction` (`:108-138`), the `FakeBackend`
  + `manager()` + `ManualClock` test harness (`:~600+` — what does the fake record? `releases()`/`configured()`;
  is there a `fail_configure` or scriptable reply? the plan adds some — judge realizability).
- `crates/bridge-core/src/ports.rs` — `AgentBackend::prompt` (`:34`, `BackendStream`); `Update` (`:21`,
  `Text`/`Usage`/`Permission`/`Done`).
- `crates/bridge-core/src/error.rs` — `AgentCrashed{reason}` (`:57`), `MessageTooLarge` (`:55`), `HandleBusy`,
  `SessionNotFound`, `ConfigInvalid`, the `disposition()` arms (`:105-121` — confirm `MessageTooLarge`/
  `AgentCrashed`→`SetState(Failed)`→`-32603`).
- `crates/bridge-core/src/domain.rs` — `Part` (`:7` — is it `{text}` only? is there a `Part::new`?).
- `crates/bridge-a2a-inbound/src/server.rs` — dispatch (`:691`), `session_clear` handler (`:2932`),
  `context_id_arg`, `warm_local_dispatch` (`:557`/`:581-591`), `LocalDispatch`, `spawn_local_producer`
  (`:1128`, `parts`→`Translator::run` `:1150`), the unary collect (`~:2292-2540`), the test `FakeBackend`(s)
  (`:3630`, `:5804` — do they ignore prompt parts?), the warm-dispatch test harness +
  `session_clear_dispatch`/`session_clear_unknown_ctx_is_not_found` (`:6509`/`:6580`).
- `crates/bridge-workflow/src/executor.rs:~131-148` — the `Update::Text`-drain precedent.
- `bin/a2a-bridge/src/main.rs` — `session_cmd` (`:2724`, `match sub` + params + missing-subcommand string),
  help (`:104`); the serve `SessionManager` construction (`:3667`); `bin/a2a-bridge/src/config.rs` server struct.

REVIEW DIMENSIONS (ground each in code):
1. **Spec faithfulness.** Does each FIX-1..14 map to a task STEP that implements it? Any scope creep / gap? Is
   the EXPIRE contract (FIX-1) actually in T2's impl + locked by T3? Is the configure-error-not-SessionExpired
   (FIX-3) correct in the T2 code?
2. **Task ordering / compile-at-each-boundary.** T1 adds `WarmTurn.seed`/`pending_seed` — does it fix ALL
   construction sites + `match SessionState` so `cargo test --workspace --no-run` passes at T1's end? Does the
   T2 builder field (`with_compact_summarize_timeout`) avoid a `new`/`new_with_clock` signature ripple (the
   plan claims so — verify against `:118-138`)? Does each later task compile against the prior?
3. **T2/T3 — `compact_session` correctness (highest risk).** Is the claim block (Idle→Compacting, capture incl.
   the fallible cwd parse BEFORE the flip, FIX-9) correct and free of an early `?` that strands `Compacting`?
   Is the `tokio::time::timeout` wrap correct (does the timeout test need `start_paused`/`tokio::time::pause`,
   or is a real short duration OK — the plan uses a 10ms real timeout + a `pending()` future; will that be
   deterministic/non-flaky)? Is the bad-summary match (Ok(empty)/Ok(Err)/Err-timeout) → EXPIRE correct? Is
   `expire_after_summarize` correct vs the `checkout_turn:276-292` tombstone (single release of `old_id`, no
   double-release, lease dropped)? Is the reset tail's revalidate + commit-or-EXPIRE faithful to
   `reset_session:475-519` (incl. FIX-3)? Does the `compact_session<F,Fut>` closure signature actually COMPILE
   in Rust (FnOnce + a returned Future; Send/'static bounds for use across `.await`; the handler's
   `|backend, session| summarize_collect(backend, session)` closure)?
4. **T4 — `summarize_collect`.** Correct full-text drain (accumulate `Update::Text`, byte-bound DURING drain →
   `MessageTooLarge`, `Permission`→`AgentCrashed`, ignore `Usage`, stop on `Done`)? Is the `ScriptedBackend`
   test fake realizable (an `AgentBackend` impl returning a `BackendStream` of scripted `Update`s via
   `futures::stream::iter`)? Is `Part { text: .. }` the right constructor (or is there a `Part::new`/more
   fields)? Are `Update`/`AgentBackend`/`Part`/`BridgeError`/`SessionId` importable in `server.rs`?
5. **T5 — seed take-and-clear.** At the TWO resume success returns ONLY (`:193-205`, `:261-275`), via
   `h.pending_seed.take()` inside the existing lock; NOT at mint/HandleBusy/reseed/Err; drop in `reset_session`
   commit (`:510-516`). Are the two tests (`checkout_consumes_seed_once`, `clear_drops_pending_seed`) writable
   on the existing harness + meaningful?
6. **T6 — seed prepend.** `LocalDispatch.seed` threaded from `warm_local_dispatch`; prepend the WRAPPED seed at
   BOTH `spawn_local_producer` (`:1138/:1150`) AND the unary collect (`~:2354`) before `Translator::run`. Is the
   recording `FakeBackend` (`prompted_parts`) realizable in the server tests, and do the `seed_prepended_*`
   tests have enough harness (they're sketched — is the sketch fleshable on `session_clear_dispatch`'s pattern)?
7. **T7 — wire/CLI/config.** `SessionCompact` dispatch arm + handler (the `compact_session(&ctx, |b,s|
   summarize_collect(b,s))` wiring) shaped like `session_clear`; result `{contextId,compacted,generation}`;
   `NotFound`→`SessionNotFound`; CLI `compact`→`SessionCompact` + the `{contextId}` params (no force) + help +
   missing-subcommand string; the `compact_summarize_timeout_secs` config knob + the
   `.with_compact_summarize_timeout(..)` serve wiring at `main.rs:3667`.
8. **TDD realizability overall.** Every new test helper (`manager_with_timeout`, `fail_configure`,
   `ScriptedBackend`, recording `prompted_parts`) — does it lean on the existing harness shape or need
   significant new scaffolding the plan under-specifies? Is any named test NOT actually writable as described?
9. **Live-gate provability** (T8) — DoD (codeword survives, throwaway gone, same pid, gen advances, usage null)
   provable on real codex via `submit`/`session compact`/`session status`?

OUTPUT: findings by severity (task #, file:line, fix); spec-faithfulness verdict; task-ordering verdict;
code-correctness verdict; live-gate verdict. End with exactly: `PLAN VERDICT: ready-to-execute | fix-then-execute | rework`.
