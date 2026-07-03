You are doing a rigorous SPEC-REVIEW (read-only) of the "Warm-turn cancellation tokens" spec — the Slice-9
prerequisite that closes two PRE-EXISTING warm-session concurrency races — grounded against the ACTUAL
post-Slice-8 a2a-bridge code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git log`; do NOT
edit/build/test. Be a decisive, adversarial reviewer: find BLOCKERS, gaps, contradictions, under-specification,
and missed concurrency hazards BEFORE this becomes a plan. The races are REAL and PRE-EXISTING — pressure-test
the FIX's correctness + executability, not whether the races exist.

The spec is below:

{{input}}

CONTEXT — VERIFY every `file:line` against the CURRENT tree (the prior follow-up doc
`docs/superpowers/2026-06-18-FOLLOWUP-warm-turn-cancellation-tokens.md` has PRE-Slice-8 anchors; Slice 8 MOVED
SessionManager + the warm-turn path into `crates/bridge-coordinator`):
- **SessionManager (now in bridge-coordinator):** `crates/bridge-coordinator/src/session_manager.rs` —
  `checkout_turn` (takes a caller `op`, `:231`), `checkout_existing_turn` (Slice-8, `:~248`), `WarmTurn`
  (`:68`), `finish_turn`/`record_usage` (the gen+op+Running guard), `reset_session`/`reset_session_inner`
  (the `Resetting` claim + `release_session`, `:796/:808`), `SessionState` (`:24`).
- **The Coordinator (Slice-8):** `crates/bridge-coordinator/src/coordinator.rs` — `prompt`/`continue_turn`/
  `collect_turn` (`:173/:193/:~205`), `mint_operation_id` (the existing unique nonce — Race 1 already closed on
  MCP), `TurnFinishGuard` (the Slice-8 drop guard).
- **Dispatch types:** `crates/bridge-coordinator/src/dispatch.rs` — `WarmTurnGuard` (keys on `turn.op`),
  `LocalDispatch`.
- **The A2A producers (still in bridge-a2a-inbound; T9 deferred):** `crates/bridge-a2a-inbound/src/server.rs` —
  the `op-{task}` derivations (`:770` streaming, `:2273` unary Local), `warm_local_dispatch` (`:488`), the
  `store.put`-before-spawn gap, the unary Local collect loop, `session_clear`/`session_compact`, the `"task-1"`
  stub (`:3187`).
- **The re-mint:** `crates/bridge-acp/src/acp_backend.rs` (lazy session re-mint on prompt) +
  `crates/bridge-core/src/translator.rs:133` (the mint path entered from `backend.prompt`).

PRESSURE-TEST (ground every finding in real code with `file:line`):
1. **F1 op-nonce unification.** Does `checkout_turn` truly take a caller `op` today, and do ALL callers
   (A2A `warm_local_dispatch` `:497`, the unary path, the Coordinator `prompt`/`collect_turn`,
   `checkout_existing_turn`) ALREADY consume `turn.op` for `finish_turn`/`record_usage` — so dropping the param
   is safe? Are there callers that use the task-derived op for ANYTHING ELSE (logging, store keys, the cancel
   routing `op`)? Does the `"task-1"` stub feed any non-warm path that still needs an op? Is minting an
   `AtomicU64` nonce inside `checkout_turn` free of a lock-ordering issue (it holds `by_context`)?
2. **F2 abort token — the re-mint timing (the load-bearing correctness claim).** Trace the A2A streaming
   producer AND the unary Local collect AND the Coordinator `collect_turn`: does wrapping the translator drive
   in `select! { biased; _ = abort.cancelled() => …, ev = events.next() => … }` ACTUALLY prevent the
   `backend.prompt` re-mint? `translator.run(...)` calls `backend.prompt` lazily on the FIRST stream poll — so
   is the select entered BEFORE that first poll on every path? If the abort fires WHILE `backend.prompt` is
   in-flight (already re-minting), does dropping the stream cancel it in time, or can the session still be
   resurrected? Is there a TOCTOU between `checkout_turn` returning the token and the producer entering the
   select?
3. **F2 cancel-under-claim ordering.** In `reset_session_inner`, is cancelling `handle.turn_abort` BEFORE
   `release_session`, under the `Resetting` claim, correct + sufficient? Does the aborted producer's
   `WarmTurnGuard::drop` → `finish_turn(OLD_gen, OLD_nonce)` correctly no-op (gen bumped + state Resetting +
   nonce mismatch)? Can the abort token leak or fire on a freed handle? For `checkout_existing_turn` (continue),
   is minting a FRESH per-turn token (replacing the handle's) correct, or does it clobber a token a concurrent
   reset is about to cancel?
4. **F2 outcome mapping.** Is "aborted turn → Canceled" well-defined on BOTH surfaces (A2A unary/streaming
   response + MCP TurnOutput.stop_reason)? Does the existing cancel path (SessionCancel) already produce a
   Canceled terminal the abort can reuse, or is a new signal needed?
5. **F3 `session_clear` spawn-detach.** Does mirroring `session_compact`'s detached-spawn pattern apply cleanly
   to `session_clear` (the claim it drives is `Resetting`, not `Compacting`)? Does spawn-detach change the
   handler's response contract (clear currently returns the ResetOutcome synchronously — does detaching it
   force a fire-and-forget, changing the wire response)? If so, is that a behavior change the spec must own?
6. **Scope / behavior-preservation.** Does any change alter a wire shape or an existing test's expectation
   (the A2A 133+47 / bridge-coordinator suites)? Is the `op-{task}` removal observable anywhere (a test that
   asserts the op string, a log field consumers grep)? Is the bundle (F1+F2+F3) right-sized for one slice, or
   should F3 split out?
7. **DoD / live-gate executability.** Are the Race-1 and Race-2 gates concretely testable as written (the
   checkout→prompt window is tiny — can a unit test deterministically hit it, e.g. a gated backend)? Is the
   live-gate (cancel-then-resend; force-clear-during-turn) runnable on both surfaces with the existing
   harnesses?

OUTPUT: a numbered findings list, each tagged `BLOCKER | MAJOR | MINOR | NIT` with a `file:line` anchor + a
CONCRETE fix. End with one line: `SPEC VERDICT: ready-to-plan | fix-then-plan | needs-rework`. Decisive +
specific. Do NOT edit any files.
