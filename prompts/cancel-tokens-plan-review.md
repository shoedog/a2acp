You are doing a rigorous PLAN-REVIEW (read-only) of the "Warm-turn cancellation tokens" implementation plan,
grounded against the ACTUAL post-Slice-8 a2a-bridge code (session-cwd = the bridge repo). READ-ONLY: read
files, grep, `git log`; do NOT edit/build/test. Be a decisive, adversarial reviewer: find BLOCKERS, gaps,
WRONG code in the steps, mis-ordered tasks, missed call sites, and broken compile-safety BEFORE implementation.
One lens of a dual plan-review. The spec rulings (v2 SPEC-FIX-1..8 + L1..L3) are SETTLED — pressure-test the
plan's FAITHFULNESS to them + its EXECUTABILITY (every code block must be correct against the real APIs).

The plan is below:

{{input}}

BINDING DOCS: spec `docs/superpowers/specs/2026-06-21-warm-turn-cancellation-tokens.md` (v2 section is binding).
VERIFY every `file:line` + every API the plan's code uses against the CURRENT tree:
- `crates/bridge-coordinator/src/session_manager.rs` — `WarmHandle` (`:46`, fields incl. `op`/`state`/
  `generation`/`backend_session`), `WarmTurn` (`:68`), `checkout_turn` (`:231`)/`checkout_turn_inner` (`:284`)/
  `checkout_existing_turn` (`:254`)/`checkout_child_turn` (`:502`, holds `children` then calls `_inner`),
  `finish_turn` (`:535`, the gen+op+Running guard), `record_usage`, `reset_session_inner` (`:808`, the claim
  block `:814-855`, `release_session` `:862`), `SessionState` (`:24`), the constructors (where `turn_op_seq`
  must init).
- `crates/bridge-coordinator/src/dispatch.rs` — `LocalDispatch` (`:71`), `WarmTurnGuard`.
- `crates/bridge-coordinator/src/coordinator.rs` — `prompt`/`continue_turn`/`collect_turn`,
  `mint_operation_id`, `TurnFinishGuard`, the `stop_reason` mapping (`completed`/`cancelled`/`failed`).
- `crates/bridge-a2a-inbound/src/server.rs` — `op-{task}` (`:770`, `:2273`), `warm_local_dispatch` (`:488`,
  the `Ok(turn)` arm builds `LocalDispatch` `:512`), `resolve_configure_bind` (`:413`, cold-bind also returns
  `LocalDispatch`), `spawn_local_producer` (`:1390`, the `select!` `:1416`, the `tx.closed()` arm), the unary
  Local loop (`:2306`, the terminal mapping `:2493`), `WarmWorkflowNodeDispatcher::checkout` (`:551`).
- `crates/bridge-core/src/translator.rs` — the `Event` API: confirm the EXACT constructor for a terminal
  Canceled event (the plan writes `Event::terminal(TaskOutcome::Canceled)` — verify it exists + the signature)
  and `EventKind`/`outcome()`.

PRESSURE-TEST (ground every finding in real code with `file:line`):
1. **Task 1 op-param removal — is the caller list COMPLETE?** Grep EVERY caller of `checkout_turn`,
   `checkout_existing_turn`, `checkout_child_turn` (src + tests + the `coordinator.rs` test fixtures
   `coordinator_fixture_with_registry`/the `checkout_turn(...)` calls at `:1261/:1017`). Does the plan miss
   any? Will the tree actually compile at the end of Task 1, or does a missed site / a test helper break it?
   Does `checkout_turn_inner` set `h.op` in MORE than one branch (the no-diff reuse branch `:276-292` AND the
   fresh-mint branch) — does the plan mint in ALL of them? Does any caller use the OLD op for logging/store
   keys/cancel-routing beyond `finish_turn`/`record_usage`?
2. **The `Event::terminal(TaskOutcome::Canceled)` API (used in Tasks 4/5/6).** Does that constructor EXIST
   with that signature? Is `TaskOutcome` in scope in `server.rs` + `coordinator.rs`? In the streaming producer,
   does sending a terminal Canceled via `tx` BEFORE `return` correctly interact with `translator_terminal` /
   the producer's clean-end→Completed mapping (no double terminal, no Completed override)? In the unary path,
   does pushing a Canceled terminal into `collected` make `:2493`'s `rev().find_map(outcome())` yield
   `TASK_STATE_CANCELED`?
3. **The biased select correctness (L1).** In `spawn_local_producer`, is making the existing select `biased;`
   with abort FIRST safe w.r.t. the `tx.closed()` arm (does abort-priority starve disconnect detection — no,
   both are edge events)? Is the abort arm's `tx.send(Canceled).await` safe if `tx` is already closed (ignore
   the error)? For the unary + `collect_turn` loops (currently plain `while let`), does the rewrite preserve
   the usage-skip + error-tracking + terminal logic EXACTLY for the non-abort path?
4. **`collect_turn` abort outcome (Task 6).** The plan says seed a Canceled terminal OR special-case
   `stop_reason="cancelled"`. Does `collect_turn` compute `stop_reason` from `events.iter().rev().find_map(
   outcome())` AFTER the loop? Does the synchronous `finish_turn` + `TurnFinishGuard.disarm()` still run on the
   abort path (it should — the handle must return to a clean state)? Is there a double-finish or a missed
   disarm?
5. **Task 2 reset cancel ordering.** Is taking `h.turn_abort.take()` in the claim block + cancelling after the
   lock, before `backend.cancel`/`release_session` (`:859-862`), correct? Does `reset_session_inner` have
   OTHER early returns inside the claim block (the `new_id` parse error `:830`, the cwd parse error `:837`)
   that would skip the cancel — and does that matter (those are pre-claim-commit failures)? Is `turn_abort`
   correctly `None` for the Idle (non-force) path so existing reset tests are unaffected?
6. **WarmTurn.abort field ripple.** Adding `abort: CancellationToken` to `WarmTurn` — does EVERY `WarmTurn {
   … }` construction site get the new field (grep them: `checkout_turn_inner`'s branches, `checkout_existing_
   turn`, any test builder)? Same for `WarmHandle.turn_abort` (every `WarmHandle { … }` construction).
7. **Scope + behavior-preservation.** Does any step touch a wire shape or an existing test's expectation
   beyond the intended op-string assertions? Is F3 truly absent? Are the "out of scope" items (ensure_session
   abort, stale store.put, MCP force) correctly NOT implemented? Is the task ordering (1→7) the right
   dependency order, and is each commit-point's tree green?

OUTPUT: a numbered findings list, each tagged `BLOCKER | MAJOR | MINOR | NIT` with a `file:line` anchor + a
CONCRETE fix (or a corrected code block). End with one line:
`PLAN VERDICT: ready-to-implement | fix-then-implement | needs-rework`. Decisive + specific. Do NOT edit files.
