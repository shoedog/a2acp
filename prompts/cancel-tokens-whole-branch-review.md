You are doing a WHOLE-BRANCH code review (read-only) of the "warm-turn cancellation tokens" implementation — the
ENTIRE `main...HEAD` diff on `feat/warm-turn-cancellation-tokens`, as ONE coherent change. This is the cross-task
safety net: per-task tests passed, but cross-task / concurrency / race / lifecycle bugs spanning commits only surface
here. This change is PURE CONCURRENCY plumbing (a per-turn cancellation token + a manager-minted op nonce) — so the
bugs that matter are subtle: a non-biased select, a stranded token on an Idle handle, a stale producer that still
matches the gen+op guard, a cancel that races a lock release. Be rigorous, adversarial, decisive. READ-ONLY: read
files, grep, `git diff main...HEAD`; do NOT edit/build/test.

Binding spec `docs/superpowers/specs/2026-06-21-warm-turn-cancellation-tokens.md` (the `## v2 (SPEC-FIX-1..8 + L1..L3)`
section is BINDING) + plan `docs/superpowers/plans/2026-06-21-warm-turn-cancellation-tokens.md` (the `## v2 (PLAN-FIX-
1..7)` section is BINDING). GROUND every finding in real code with `file:line`.

WHAT THIS CHANGE DOES (two races, F1 + F2; F3 is OUT of scope — do not flag its absence):
- **F1 — Race 1 (cancel→next-turn op collision):** `SessionManager` now mints a UNIQUE op nonce (`turn-{n}` via an
  `AtomicU64`) per checkout, REPLACING the caller-supplied `op` param (the old A2A `op-{task}` derivations at
  `server.rs` and the child dispatcher are DELETED). The `finish_turn`/`record_usage` gen+op guard then discriminates
  turns even WITHOUT a generation bump — so a late-completing cancelled turn can't clobber the next turn's bookkeeping.
- **F2 — Race 2 (`clear --force` vs producer-start → context resurrection via ACP lazy re-mint):** a per-turn
  `CancellationToken` rides the handle (`WarmHandle.turn_abort: Option<...>`) → `WarmTurn.abort` → `LocalDispatch.abort`.
  Producers race it in a **`biased;` select with the abort arm FIRST**, entered BEFORE the first translator poll (the
  first poll is what lazily calls `backend.prompt`, which re-mints the just-cleared ACP context). `reset_session(force)`
  cancels the token under the `Resetting` claim, BEFORE releasing the session lock.

{{input}}

REVIEW THE WHOLE BRANCH (`git diff main...HEAD`) for — pressure-test EACH of these (they are the known risk surface):

1. **THE RE-MINT GUARANTEE (the whole point of F2).** Is the abort select `biased;` with the abort arm FIRST in ALL
   THREE warm producers — (i) the streaming `spawn_local_producer` (`server.rs`), (ii) the unary Local handler
   (`server.rs`), (iii) the Coordinator `collect_turn` (`coordinator.rs`)? A non-biased select, or the abort arm
   placed second, means a ready `events.next()` can win the race and drive the FIRST poll → `backend.prompt` re-mints
   the cleared context (resurrection). For each: confirm the select is entered BEFORE any `.next()`/poll that could
   trigger `backend.prompt`, and that on the abort arm it emits a `Canceled` terminal and returns WITHOUT polling.

2. **TOKEN LIFECYCLE — no stranded token, no double-cancel.** Is `turn_abort` cleared on EVERY turn-end path so an
   Idle handle never carries a stale (possibly already-cancelled) token into the NEXT turn? Check: `finish_turn`
   (normal + error exit), `cancel_inner` (the `take()` then cancel), `reset_session_inner` (the `take()` under the
   claim), and the compact / clear new-generation handle construction. Conversely, is the token cancelled AT MOST the
   right number of times — `take()` (not clone-and-leave), so a later path can't re-cancel a token a new turn now owns?
   Can two paths (e.g. `cancel_task` + `reset_session(force)`) both grab and act on the token? Is `finish_turn`'s clear
   gen+op-guarded so it doesn't clear a token that a DIFFERENT (newer) turn already installed?

3. **THE NONCE GUARD (gen+op) — can a stale producer still match?** With the op now a manager nonce, walk the
   `finish_turn`/`record_usage` guard: does it compare BOTH generation AND op, and reject when EITHER differs? Can a
   late callback from a cancelled/superseded turn (same generation, old nonce) still pass the guard and mutate state
   (write usage, flip status, release a claim)? Is the nonce monotonic and never reused within a handle's life? Is
   the `AtomicU64` ordering adequate (it's single-manager-locked — is the atomic even load-bearing, or is it under the
   lock anyway)? Confirm `record_usage` uses the SAME guard.

4. **`reset_session_inner` CANCEL ORDERING.** The cancel must happen under the `Resetting` claim and BEFORE
   `release_session` — confirm the `let turn_abort = h.turn_abort.take();` is captured inside the locked claim block
   and the `.cancel()` fires before the lock is released / the session is released, so a producer can't observe the
   new empty generation AND an un-cancelled token. Any window where the producer's first poll interleaves between the
   generation bump and the cancel?

5. **THE ~100-SITE OP-PARAM SWEEP (move integrity).** The `op` param was dropped from `checkout_turn` /
   `checkout_existing_turn` / `checkout_child_turn` and every post-checkout guard switched to `&turn.op`. Verify NO
   caller still passes a stale/derived op, NO guard compares against a now-wrong value, and the deletion of the A2A
   `op-{task}` derivations + the child dispatcher's derivation left NO dangling reference or behavior change. Did any
   call site that USED the old deterministic `op-{task}` (e.g. for idempotency/lookup) silently lose that property?

6. **BYTE-IDENTITY ELSEWHERE.** The cold (non-warm) dispatch, the API backend, the workflow/detached paths, and the
   A2A unary/session/cancel handlers should be behaviorally UNCHANGED except for the token plumbing (cold-bind installs
   a FRESH `CancellationToken::new()`). Confirm the 133 lib + 47 integration bridge-a2a-inbound tests stay
   byte-identical in intent. Is the cold-bind token actually wired so a cold turn is also cancellable, or is it inert?

7. **ABORT-ARM SEND SAFETY + TERMINAL CORRECTNESS.** On the abort arm, the producer does `tx.send(Canceled terminal)`
   — if the consumer already dropped the channel (client gone), is the `send` error IGNORED (not `unwrap`/`?` →
   panic/early-return that skips cleanup)? Does the Coordinator `collect_turn` abort path `drop(events)` BEFORE
   `finish_turn` (PLAN-FIX-6) and push a `Canceled` terminal so `stop_reason == "cancelled"`? Do all three producers
   converge on the SAME terminal outcome (`TaskOutcome::Canceled` / `TASK_STATE_CANCELED`)?

8. **CROSS-CUTTING.** Any lock held across `.await` introduced by the new token handling (deadlock)? Any
   `unwrap`/`expect`/panic on a request-driven path? Any `CancellationToken` cloned where a `child_token()` or a fresh
   token was meant (sharing cancellation across turns)? Does `dispatch.rs`'s new `abort` field default sensibly
   everywhere it's constructed? Any test that now passes for the WRONG reason (asserts Canceled but the abort arm
   never actually fired)?

OUTPUT: a numbered findings list, each tagged `BLOCKER | MAJOR | MINOR | NIT` with a `file:line` anchor + a CONCRETE
fix. Focus on the re-mint guarantee, token-lifecycle, the gen+op guard, and cancel-ordering races a single-file review
misses. End with one line: `BRANCH VERDICT: approve | approve-with-nits | changes-required`. Do NOT edit files.
