# Spec — Warm-turn cancellation tokens (Slice-9 prerequisite)

> **Status:** v2 — dual spec-review folded (codex-xhigh `needs-rework` + Opus lens). The **v2 fixes
> (SPEC-FIX-1..8 + L1..L3) below are BINDING and SUPERSEDE any contradicting text in the original draft.**

## v2 — dual-review folded (BINDING)

**Scope change — F3 SPLIT OUT.** F3 (`session_clear` strand-window hardening) is REMOVED from this slice.
codex (BLOCKER-4) showed it's not a simple `reset_session` spawn-detach: `session_clear` runs
`clear_with_children` while **holding the `workflow_runs` lock** across the sweep (an atomicity guard with an
explicit test at `server.rs:7337`), so naive spawn-detach would skip children AND reopen the run-start race.
Combined with my L3 (detaching changes clear's synchronous wire contract), F3 is its own non-trivial hardening
→ **defer to a separate commit/slice.** This slice is **F1 (op nonce) + F2 (abort token) only.**

**SPEC-FIX-1 (BLOCKER — abort token must REACH the A2A producers).** `warm_local_dispatch` collapses
`WarmTurn` into `LocalDispatch` (`server.rs:512`), dropping everything but backend/session/seed/guards — so
`turn.abort` never reaches `spawn_local_producer`/the unary loop. FIX: add `abort: CancellationToken` to
`LocalDispatch`, populate it from `WarmTurn` in `warm_local_dispatch`, and have BOTH A2A local loops select on
that exact token.

**SPEC-FIX-2 (BLOCKER — F1 must cover `checkout_child_turn`).** There is a THIRD warm checkout entry point,
`checkout_child_turn` (`session_manager.rs:502`; `WarmWorkflowNodeDispatcher` derives
`workflow-{run_id}-node-{id}`, `server.rs:551`). For "every warm turn gets a manager-minted nonce" to hold,
F1 must drop the caller `op` from `checkout_child_turn` too and mint internally (update child-turn tests).

**SPEC-FIX-3 (BLOCKER — Race 2 live-gate is A2A-ONLY).** MCP `clear` has no `force` (Coordinator.clear hardcodes
`false`), and Race 2 is force-only. The MCP-surface force-clear DoD is impossible without an API change (out of
scope). FIX: Race 2 is live-gated on the **A2A surface only**; MCP gets the Race-1 (op nonce) + the abort-token
benefit, but force-clear stays A2A. (Adding `force` to MCP clear is a later, separate decision.)

**SPEC-FIX-4 (BLOCKER) → folded into the F3 split above** (do not bundle the `workflow_runs`-lock migration).

**SPEC-FIX-5 (MAJOR — narrow the re-mint claim).** The biased select prevents the re-mint only for a
**pre-first-poll** abort. Once the first poll enters `backend.prompt`, ACP runs `ensure_session` (may already
send `session/new`) before returning the stream (`acp_backend.rs:1955`). The REAL Race-2 window (the
`store.put().await` gap between checkout and the producer's first poll) IS covered by pre-first-poll abort
(biased select catches it). FIX: the spec claims ONLY "pre-first-poll abort prevents the re-mint"; an
abort landing mid-`ensure_session` is a narrower sub-window — documented as a known limitation (full coverage
would need a backend-side abort inside `ensure_session`, out of scope this slice).

**SPEC-FIX-6 (MAJOR — abort must emit Canceled, not fall through to Completed).** All three drive loops default
"no terminal → Completed" (Coordinator `collect_turn`; unary A2A `server.rs:2495`; streaming A2A appends
Completed unless `translator_terminal`, `:1458`). FIX: on abort, EXPLICITLY set `TaskOutcome::Canceled` in all
three; tests assert Canceled (not Completed) for an abort-before-first-poll.

**SPEC-FIX-7 (MAJOR — post-clear DoD).** A successful (non-force) clear KEEPS the handle, swaps to a new empty
generation, leaves it Idle — so post-clear `continue` does NOT return `SessionNotFound`; it exercises the empty
new generation. FIX: the Race-2 DoD asserts **generation/session rollover + empty recalled context** (recall=
none), NOT `SessionNotFound` (which is only for a truly unknown context).

**SPEC-FIX-8 (MAJOR — stale `store.put` is out of scope).** F2 stops the prompt but not an already-awaited
`store.put(task, old_session)` in the checkout→spawn window; a delayed stale write could clobber a newer
same-task mapping after force-clear+resend. FIX: this slice closes the **context-resurrection** harm (the
re-mint); the task→session map staleness on a force-clear-then-same-task-resend is **explicitly out of scope**
(documented follow-up — a gen/op-aware store update). Note it; do not silently imply it's covered.

---

> **Status (original draft, below — superseded where it conflicts with v2):** DRAFT for dual spec-review. Closes the two PRE-EXISTING warm-session concurrency races tracked in
> `docs/superpowers/2026-06-18-FOLLOWUP-warm-turn-cancellation-tokens.md`, **re-grounded against the post-Slice-8
> tree** (SessionManager + the warm-turn path now live in `crates/bridge-coordinator`). This is the gating
> prerequisite for **Slice 9** (Turn Channel + E2 permission — a cancel-under-concurrency feature). Bundles the
> tracked Slice-4 `session_clear` spawn-detach hardening (same clear/`force` path).

## Goal

Make warm-session cancel/`force` race-free under concurrency by giving every warm turn (a) a **manager-minted
unique operation nonce** (so a stale producer's late completion cannot clobber a new turn) and (b) a **per-turn
abort token** (so `clear --force` aborts an in-flight producer instead of racing it / resurrecting the cleared
context). Plus: **spawn-detach `session_clear`** so a dropped caller future cannot strand the `Resetting` claim.

Non-goal: true mid-turn injection (Slice 9). Non-goal: changing any A2A/MCP wire shape (behavior-preserving
except the two races + the strand window close).

## Background — the two races (PRE-EXISTING; verified in the current tree)

### Race 1 — cancel→next-turn op collision
`OperationId` is **task-derived** on the A2A edge: `op-{routed.task}` at `server.rs:770` (streaming producer)
and `server.rs:2273` (unary Local). The generation guard (`finish_turn`/`record_usage` no-op unless
`gen == handle.generation && op == handle.op && state == Running`, `session_manager.rs`) cannot discriminate
two turns that share an op:
- A `SessionCancel` then a same-context send with the **same or omitted** `taskId` reuses `op-{task}` (omitted →
  the `"task-1"` stub, `server.rs:3187`). A plain cancel does NOT bump the generation, so the cancelled
  producer's late `finish_turn`/`record_usage` still satisfies `gen && op && Running` → it can idle/clobber the
  **new** turn.

**Slice-8 interaction (NARROWS this to the A2A surface):** the `Coordinator` (MCP/CLI) already mints a unique
op via `mint_operation_id()` (`coordinator.rs`), so Race 1 is **already closed on the MCP surface**. It survives
only on the A2A `op-{task}` path. The fix unifies both onto one manager-minted nonce.

### Race 2 — `clear --force` vs producer start (context resurrection)
`checkout_turn` marks the handle `Running`, but the A2A handlers `await store.put(...)` **before** the producer
prompts. In that window a concurrent `SessionClear { force: true }` claims (`Running`→`Resetting`) and
**releases** the old bridge `SessionId` (`reset_session_inner` → `backend.release_session`). The original
producer then prompts the **released** session, which ACP **lazy re-mints** → **resurrects the force-cleared
context** the operator just wiped. (`clear` without `force` requires `Idle`, so it has no in-flight turn to race —
Race 2 is `force`-only.)

## Design

### F1 — Manager-minted unique op nonce (closes Race 1, both surfaces)
- `SessionManager` owns a process-unique monotonic counter (`AtomicU64`, e.g. `turn-{n}` minted as an
  `OperationId`). `checkout_turn` and `checkout_existing_turn` MINT the nonce internally and set
  `handle.op = Some(nonce)`; they **no longer accept a caller `op` param**. `WarmTurn.op` carries the nonce
  (already a field).
- **Callers already use `turn.op`** for the guard/usage — verified: A2A `warm_local_dispatch` builds
  `WarmTurnGuard { op: turn.op.clone() }` (`server.rs:521`); the unary Local path records usage via the
  warm_guard's `op` (`w.op`); the Coordinator's `collect_turn` uses `turn.op`. So dropping the param requires
  only: delete the `op-{task}` derivations (`server.rs:770, 2273`) and the Coordinator's `mint_operation_id`
  call sites that feed checkout (the standalone `mint_operation_id` may be retained for task-id minting or
  removed if unused). `finish_turn`/`record_usage` signatures (which take an `&OperationId`) are unchanged —
  they receive `turn.op` (the nonce) as today.
- Result: a cancelled producer's stale `finish_turn(ctx, gen, OLD_nonce)` cannot match the new turn (distinct
  nonce) even when the generation is unchanged (plain cancel).

### F2 — Per-turn abort token (closes Race 2; makes `force` abortive)
- `SessionManager` mints a `tokio_util::sync::CancellationToken` per `checkout_turn`/`checkout_existing_turn`,
  stores it on the handle (`handle.turn_abort`), and returns a clone on `WarmTurn` (new field
  `abort: CancellationToken`).
- **The producers select on it AROUND the translator drive**, entered **before the first stream poll** (so an
  abort that lands before `backend.prompt` runs prevents the re-mint, and one landing during interrupts it):
  - A2A streaming + unary Local: the `translator.run(...)` event loop is wrapped
    `tokio::select! { biased; _ = abort.cancelled() => <abort>, ev = events.next() => <handle> }`.
  - Coordinator `collect_turn`: same select around its drain loop.
  - On abort: stop consuming, treat the turn as Canceled (emit the terminal Canceled outcome / map to the
    surface's cancel response), and DROP the translator stream (cancelling the in-flight `backend.prompt`).
- **`reset_session_inner` cancels the handle's `turn_abort` under the `Resetting` claim**, BEFORE
  `backend.release_session`. Ordering: claim `Running`→`Resetting` (only reachable with `force`) → cancel the
  token → release. The aborted producer's `WarmTurnGuard::drop` then `finish_turn(ctx, OLD_gen, OLD_nonce)`
  no-ops (generation bumped + state not Running + nonce mismatch — defense in depth).
- Disabled/non-force paths unchanged: a require-`Idle` reset has no `Running` turn, so there is no token to
  cancel; the token is simply dropped when the turn finishes.

### F3 — `session_clear` spawn-detach (bundled Slice-4 hardening)
`session_clear` currently `await`s `reset_session` directly; a dropped caller future (client disconnect /
request-timeout) strands the `Resetting` claim (reap skips claimed states). Mirror `session_compact`: run
`reset_session` on a DETACHED `tokio::spawn` that always drives to commit-or-recover, so the claim cannot be
stranded by a caller drop. (Tiny window — local release+configure — but free to close alongside F2.)

## Files (anticipated; verify at plan time)
- `crates/bridge-coordinator/src/session_manager.rs` — the op-nonce counter + mint in `checkout_turn`/
  `checkout_existing_turn` (drop the `op` param); the `turn_abort` token on the handle + on `WarmTurn`; cancel
  in `reset_session_inner` under the `Resetting` claim.
- `crates/bridge-coordinator/src/coordinator.rs` — `collect_turn` select-on-abort; drop the checkout `op` arg.
- `crates/bridge-coordinator/src/dispatch.rs` — `WarmTurnGuard` unchanged (already keys on `turn.op`).
- `crates/bridge-a2a-inbound/src/server.rs` — delete the `op-{task}` derivations; the streaming + unary Local
  producers select on `turn.abort`; `session_clear` spawn-detach.

## DoD / live-gate
- **Race 1 (unit, manager-level):** two checkouts on the same context across a cancel (no generation bump) get
  DISTINCT nonces; the first turn's late `finish_turn`/`record_usage` no-ops against the second.
- **Race 2 (unit + live):** a `clear --force` fired in the checkout→prompt window leaves the context CLEARED
  (a subsequent `continue`/recall finds nothing / `SessionNotFound`), never resurrected; the in-flight producer
  ends Canceled, the released session is not re-minted.
- **F3:** a caller-future drop mid-`session_clear` does not strand the `Resetting` claim (the detached task
  drives to terminal); reap eventually returns the context to a clean state.
- Existing bridge-coordinator + bridge-a2a-inbound suites pass (behavior-preserving elsewhere). Live-gate vs
  real codex: cancel-then-immediate-resend (Race 1) + force-clear-during-turn (Race 2) on the A2A surface
  AND the MCP surface (`a2a-bridge mcp`).

## Opus-lens review findings (pre-folded; codex lens pending)
- **L1 (BLOCKER-class design constraint): the abort `select!` MUST be `biased;` with the abort arm FIRST.**
  This is the Slice-7b watchdog lesson. The streaming producer `spawn_local_producer` ALREADY selects
  `events.next()` vs `tx.closed()` (`server.rs:1416`) but is **NOT biased**. To deterministically prevent the
  `backend.prompt` re-mint when the abort is already pending at producer start (it can fire during the
  `store.put().await` gap, BEFORE the producer's first poll), the first poll must be a biased select that
  checks `abort.cancelled()` first. Add the abort arm there; make it `biased;` (abort arm first, then
  `tx.closed()`, then `events.next()`).
- **L2: the unary Local path (`server.rs:2316`) and the Coordinator `collect_turn` are plain `while let Some(ev)
  = events.next().await` — NO select.** They must be converted to the same `biased;` abort-select loop. (The
  streaming path already has the select scaffold; these two do not.)
- **L3: F3 spawn-detach changes `session_clear`'s response contract.** `session_clear` returns `ResetOutcome`
  (`Cleared{generation}`/`NotFound`) SYNCHRONOUSLY today; `session_compact` is async-by-nature (long summarize)
  so detaching it was free. Detaching `session_clear` makes it fire-and-forget → a WIRE CHANGE. Options: (a)
  KEEP clear synchronous + accept the tiny strand window (documented), (b) use a drop-guard (the Slice-8
  `TurnFinishGuard` pattern) that completes the reset on caller-drop WITHOUT changing the response, or (c)
  detach + change the contract. **Lean (b) or (a); do NOT silently change the wire contract.** Consider
  SPLITTING F3 to its own commit so the F1/F2 race fix isn't coupled to a clear-response decision.

## Open questions for review
1. **Abort outcome mapping:** should an aborted turn surface as A2A `Canceled` (TaskOutcome::Canceled) uniformly,
   or distinguish force-clear-abort from a client cancel? (Lean: Canceled, same as a user cancel.)
2. **`checkout_existing_turn` (continue) abort token:** continue reuses an existing handle — does it mint a NEW
   token per turn (replacing the handle's) or reuse? (Lean: mint fresh per turn; the token is per-turn, not
   per-handle.)
3. **Op-nonce format / collision-freedom:** `AtomicU64` monotonic is process-unique; is a restart-stable form
   needed (durable tasks)? (Lean: no — the nonce guards an in-memory warm handle; it dies with the process.)
4. **Re-mint timing (F2 core):** confirm the `select!`-before-first-poll guarantee actually prevents the
   `backend.prompt` re-mint in the streaming path (the producer structure differs from the unary collect).
5. **Bundle scope:** is F3 (`session_clear` spawn-detach) in-scope here, or a separate trivial commit?
