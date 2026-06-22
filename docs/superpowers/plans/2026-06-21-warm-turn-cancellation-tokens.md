# Warm-turn cancellation tokens — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Close the two pre-existing warm-session concurrency races — Race 1 (cancel→next-turn op collision)
and Race 2 (`clear --force` vs producer-start context resurrection) — by giving every warm turn a
manager-minted unique op nonce and a per-turn abort token.

**Architecture:** `SessionManager` (in `bridge-coordinator`) mints, per checkout, a unique `OperationId`
nonce and a `CancellationToken`, stored on the warm handle and returned on `WarmTurn`. The op nonce makes the
`finish_turn`/`record_usage` generation+op guard discriminate turns even without a generation bump (closes
Race 1). The abort token is threaded to every turn-driving producer, which races it in a **`biased;`**
`select!` before/while polling the translator stream; `reset_session(force)` cancels it under the `Resetting`
claim so a force-clear aborts the in-flight producer before it re-mints the released session (closes Race 2).

**Tech Stack:** Rust, `tokio`, `tokio_util::sync::CancellationToken`, `async-stream` translator.

**Binding spec:** `docs/superpowers/specs/2026-06-21-warm-turn-cancellation-tokens.md` — the **v2 section
(SPEC-FIX-1..8 + L1..L3) SUPERSEDES** the draft. **F3 is SPLIT OUT (not in this plan).** Scope = F1 + F2.

---

## File Structure
- `crates/bridge-coordinator/src/session_manager.rs` — the `turn_op_seq` counter; mint nonce + abort token in
  `checkout_turn_inner` (covers `checkout_turn` + `checkout_child_turn`) and `checkout_existing_turn`; drop the
  caller `op` param from all three; `WarmHandle.turn_abort`; `WarmTurn.abort`; cancel in `reset_session_inner`.
- `crates/bridge-coordinator/src/dispatch.rs` — `LocalDispatch.abort` field.
- `crates/bridge-coordinator/src/coordinator.rs` — `collect_turn` biased abort-select + Canceled outcome; drop
  the `mint_operation_id`-into-checkout call.
- `crates/bridge-a2a-inbound/src/server.rs` — delete the `op-{task}` derivations; `warm_local_dispatch` drops
  the `op` param + populates `LocalDispatch.abort`; the streaming producer (`spawn_local_producer`) + the
  unary Local loop biased abort-select + Canceled outcome.

## Conventions
- `OperationId`/`ContextId` are parse-don't-validate newtypes; `OperationId::parse(format!("turn-{n}"))` is
  valid. `std::sync::atomic::AtomicU64` for the counter (the manager already uses `Ordering::Relaxed` seqs).
- Lock order is unchanged: `children → by_context`. The nonce/token mint happens INSIDE the existing
  `by_context` hold in `*_inner`/`checkout_existing_turn` — no new lock.
- `biased;` select with the abort arm FIRST (the Slice-7b watchdog lesson — L1).

---

## Task 1: Manager mints the op nonce + abort token (F1 + F2 manager side)

**Files:**
- Modify: `crates/bridge-coordinator/src/session_manager.rs`
- Modify (callers, same task — keep the tree compiling): `crates/bridge-coordinator/src/coordinator.rs`,
  `crates/bridge-a2a-inbound/src/server.rs`

- [ ] **Step 1: Write the failing test** (`session_manager.rs` `#[cfg(test)]`):

```rust
#[tokio::test]
async fn checkout_mints_unique_op_nonce_per_turn() {
    let mgr = test_manager(); // existing test helper that builds a SessionManager with a fake registry
    let ctx = ContextId::parse("ctx-nonce").unwrap();
    let agent = AgentId::parse("codex").unwrap();
    let t1 = mgr.checkout_turn(&ctx, agent.clone(), None, None).await.unwrap();
    mgr.finish_turn(&ctx, t1.generation, &t1.op).await; // back to Idle
    let t2 = mgr.checkout_turn(&ctx, agent, None, None).await.unwrap();
    assert_ne!(t1.op, t2.op, "each checkout must mint a distinct op nonce");
    // A stale finish on t1.op must NOT idle the live t2 turn:
    mgr.finish_turn(&ctx, t2.generation, &t1.op).await;
    assert_eq!(mgr.status(&ctx).await.unwrap().state, "running");
}
```

- [ ] **Step 2: Run → FAIL** (`checkout_turn` still takes an `op` arg / no `abort` field).
Run: `cargo test -p bridge-coordinator checkout_mints_unique_op_nonce_per_turn`
Expected: compile error (arg count) then assertion.

- [ ] **Step 3: Implement.**
  - Add to `WarmHandle` (after `op`): `turn_abort: Option<tokio_util::sync::CancellationToken>,`.
  - Add to `WarmTurn` (after `op`): `pub abort: tokio_util::sync::CancellationToken,`.
  - Add a field to `SessionManager`: `turn_op_seq: std::sync::atomic::AtomicU64,` (init `AtomicU64::new(1)`
    in every constructor — `new_with_clock`).
  - Add a private mint helper:
```rust
fn mint_turn_op(&self) -> OperationId {
    let n = self.turn_op_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    OperationId::parse(format!("turn-{n}")).expect("minted turn op is non-empty")
}
```
  - **`checkout_turn_inner`**: remove the `op: OperationId` param. Where it currently sets the handle Running
    + `h.op = Some(op)` (the no-diff reuse branch AND the fresh-mint branch — every place it sets `h.op`/
    builds a `WarmTurn`), instead: `let op = self.mint_turn_op(); let abort = CancellationToken::new();
    h.op = Some(op.clone()); h.turn_abort = Some(abort.clone());` and put `op`, `abort` on the returned
    `WarmTurn`. (Initialize `turn_abort: None` wherever a fresh `WarmHandle` is constructed.)
  - **`checkout_turn`** + **`checkout_child_turn`**: remove the `op: OperationId` param from their signatures
    (they only forwarded it to `_inner`).
  - **`checkout_existing_turn`**: remove its `op` param; mint internally (`let op = self.mint_turn_op();
    let abort = CancellationToken::new(); h.op = Some(op.clone()); h.turn_abort = Some(abort.clone());`),
    return `op`/`abort` on the `WarmTurn`.
  - **Callers (drop the now-removed arg + the dead op derivations):**
    - `coordinator.rs` `prompt`: delete `let op = self.mint_operation_id();` feeding checkout; call
      `checkout_turn(&ctx, agent, Some(p.agent_override()), cwd)`. `continue_turn`: delete its
      `mint_operation_id`; call `checkout_existing_turn(&ctx)`. (Keep `mint_operation_id` only if still used
      elsewhere — if not, delete it.)
    - `server.rs:770` + `:2273`: delete `let op = OperationId::parse(format!("op-{}", routed.task...))...;`
      and drop the `op` arg from the `warm_local_dispatch(&srv, agent_id, &routed)` calls.
    - `server.rs` `warm_local_dispatch`: remove the `op: OperationId` param; call
      `sm.checkout_turn(&ctx, agent_id.clone(), routed.overrides.clone(), routed.session_cwd.clone())`.
    - `WarmWorkflowNodeDispatcher::checkout` (`server.rs:~551`): drop the `workflow-{run_id}-node-{id}` op
      derivation + the `op` arg to `checkout_child_turn`.
  - **Update existing tests** that pass a caller `op` to any checkout fn (manager tests + A2A test helpers)
    and any that assert an `op-{task}`/`workflow-...` op string → assert "distinct minted nonce" instead.

- [ ] **Step 4: Run → PASS** + the whole crate. Run: `cargo test -p bridge-coordinator` then
`cargo test -p bridge-a2a-inbound --no-run` (the signature ripple is a compile gate). Expected: green.

- [ ] **Step 5: Commit.** `feat(coordinator): manager-minted unique op nonce + per-turn abort token (cancel-tokens F1)`

---

## Task 2: Cancel the abort token on force-reset (F2 manager side)

**Files:** Modify: `crates/bridge-coordinator/src/session_manager.rs`

- [ ] **Step 1: Write the failing test:**

```rust
#[tokio::test]
async fn force_reset_cancels_the_inflight_turn_abort() {
    let mgr = test_manager();
    let ctx = ContextId::parse("ctx-abort").unwrap();
    let agent = AgentId::parse("codex").unwrap();
    let turn = mgr.checkout_turn(&ctx, agent, None, None).await.unwrap(); // handle now Running
    assert!(!turn.abort.is_cancelled());
    let out = mgr.reset_session(&ctx, ResetOpts { force: true }).await.unwrap();
    assert!(matches!(out, ResetOutcome::Cleared { .. }));
    assert!(turn.abort.is_cancelled(), "force reset must cancel the in-flight turn's abort token");
}
```

- [ ] **Step 2: Run → FAIL** (token not cancelled). Run: `cargo test -p bridge-coordinator force_reset_cancels`

- [ ] **Step 3: Implement** in `reset_session_inner`'s claim block (`session_manager.rs:814-855`): inside the
`by_context` hold, after `h.state = SessionState::Resetting;`, take the token out:
`let turn_abort = h.turn_abort.take();` — add `turn_abort` to the tuple returned from the block. Then,
immediately after the block (before `backend.cancel`/`release_session` at `:859-862`):
```rust
// F2: abort an in-flight producer BEFORE releasing its session so it cannot re-mint the released
// (force-cleared) context. Only the force/Running path has a live turn; an Idle reset's token is None.
if let Some(tok) = &turn_abort {
    tok.cancel();
}
```
(Add `turn_abort` to the destructured `let (backend, old_id, …, turn_abort) = { … };`.)

- [ ] **Step 4: Run → PASS** + `cargo test -p bridge-coordinator`. Expected: green (existing reset tests
unaffected — Idle resets have `turn_abort: None`).

- [ ] **Step 5: Commit.** `feat(coordinator): force-reset cancels the in-flight turn abort token (cancel-tokens F2)`

---

## Task 3: Thread the abort token into LocalDispatch (SPEC-FIX-1)

**Files:** Modify: `crates/bridge-coordinator/src/dispatch.rs`, `crates/bridge-a2a-inbound/src/server.rs`

- [ ] **Step 1:** No new unit test (struct plumbing; covered by Task 4/5's producer tests). Confirm baseline
green: `cargo test -p bridge-a2a-inbound --no-run`.

- [ ] **Step 2: Implement.**
  - `dispatch.rs` `LocalDispatch`: add `pub abort: tokio_util::sync::CancellationToken,`.
  - `server.rs` `warm_local_dispatch` (`:512`): in the `Ok(turn)` arm, add `abort: turn.abort.clone()` to the
    `LocalDispatch { … }` it builds (the field is `turn.abort`).
  - `resolve_configure_bind` (the cold-bind fallback, `:413`) also returns a `LocalDispatch` — give it a
    `CancellationToken::new()` (a cold-bind turn has no warm handle; a fresh never-cancelled token is correct —
    cold-bind is not subject to force-clear-resurrection, it has no warm session to release).

- [ ] **Step 3: Run → PASS** `cargo test -p bridge-a2a-inbound --no-run` (compiles). Commit folded into Task 4.

---

## Task 4: Biased abort-select in the A2A streaming producer

**Files:** Modify: `crates/bridge-a2a-inbound/src/server.rs` (`spawn_local_producer` `:1390-1475`)

- [ ] **Step 1: Write the failing test** (a producer-level test with a gated fake backend that blocks in
`prompt`, plus an abort token cancelled mid-flight; assert the producer emits a Canceled terminal and stops).
Model it on the existing `spawn_local_producer` tests in `server.rs` (search `spawn_local_producer` tests);
construct a `LocalDispatch` with a `CancellationToken`, cancel it, drive the producer, assert the `tx` stream
yields a Canceled terminal and ends.

- [ ] **Step 2: Run → FAIL** (no abort arm).

- [ ] **Step 3: Implement.** In `spawn_local_producer`, capture `let abort = dispatch.abort;` before the
spawn. Make the existing `tokio::select!` (`:1416`) **`biased;`** with the abort arm FIRST:
```rust
let ev = tokio::select! {
    biased;
    _ = abort.cancelled() => {
        // F2: a concurrent force-clear aborted this turn before/while it could prompt the released
        // session. Emit a terminal Canceled and stop (do NOT poll events → no re-mint when pre-first-poll).
        let _ = tx.send(Ok(Event::terminal(TaskOutcome::Canceled))).await;
        return;
    }
    _ = tx.closed() => { return; }
    maybe = events.next() => match maybe { Some(ev) => ev, None => break },
};
```
(Use the real `Event::terminal(TaskOutcome::Canceled)` constructor — verify its name in `translator.rs`/the
event API; `translator_terminal` handling already prevents a double terminal.)

- [ ] **Step 4: Run → PASS** + `cargo test -p bridge-a2a-inbound`. Expected: green (existing streaming tests
unaffected — their tokens are never cancelled).

- [ ] **Step 5: Commit.** `feat(a2a): biased abort-select in the streaming + unary warm producers (cancel-tokens F2)`
(commit after Task 5).

---

## Task 5: Biased abort-select in the A2A unary Local loop

**Files:** Modify: `crates/bridge-a2a-inbound/src/server.rs` (the unary Local collect, `:2306-2328`)

- [ ] **Step 1: Write the failing test** — unary Local with a gated backend + a pre-cancelled abort token;
assert the collected result's terminal state is `TASK_STATE_CANCELED` (the unary path maps the terminal via
`events.iter().rev().find_map(|e| e.outcome())`, `:2493`).

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement.** The unary path holds `dispatch.warm_guard`; also bind `let abort =
dispatch.abort;`. Convert the `while let Some(ev) = events.next().await {` loop to:
```rust
loop {
    let ev = tokio::select! {
        biased;
        _ = abort.cancelled() => { collected.push(Ok(Event::terminal(TaskOutcome::Canceled))); break; }
        maybe = events.next() => match maybe { Some(ev) => ev, None => break },
    };
    // ... existing usage-skip + collected.push(ev) body ...
}
```
The existing terminal-mapping (`:2493`) then yields `TASK_STATE_CANCELED`.

- [ ] **Step 4: Run → PASS** + `cargo test -p bridge-a2a-inbound`.

- [ ] **Step 5: Commit** (Task 4 + 5 together).

---

## Task 6: Biased abort-select + Canceled in the Coordinator `collect_turn`

**Files:** Modify: `crates/bridge-coordinator/src/coordinator.rs` (`collect_turn`)

- [ ] **Step 1: Write the failing test** (`coordinator.rs` tests): a `DeltaBackend`/gated backend; cancel the
turn's abort mid-drain; assert `TurnOutput.stop_reason == "cancelled"`. (Reuse the `dropped_turn_*` test
scaffolding; here cancel via the abort token rather than dropping the future.)

```rust
#[tokio::test]
async fn collect_turn_aborts_to_cancelled() {
    // gated backend; spawn prompt; cancel the handle's turn_abort; assert stop_reason == "cancelled".
}
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement.** `collect_turn` already receives the `WarmTurn` (`turn.abort`). Convert its drain
`while let Some(ev) = events.next().await {` to a biased select:
```rust
let aborted = loop {
    let ev = tokio::select! {
        biased;
        _ = turn.abort.cancelled() => break true,
        maybe = events.next() => match maybe { Some(ev) => ev, None => break false },
    };
    // ... existing usage-record + collected.push body ...
};
```
After the loop, finish synchronously + disarm the `TurnFinishGuard` as today. Then on `aborted`, force the
outcome: compute `stop_reason` as `"cancelled"` when `aborted` (don't fall through to the
`TaskOutcome::Completed | None => "completed"` default — SPEC-FIX-6). E.g. seed `collected` with an
`Event::terminal(TaskOutcome::Canceled)` on abort, or special-case `if aborted { stop_reason = "cancelled" }`.

- [ ] **Step 4: Run → PASS** + `cargo test -p bridge-coordinator`.

- [ ] **Step 5: Commit.** `feat(coordinator): collect_turn biased abort-select -> Canceled (cancel-tokens F2)`

---

## Task 7: DoD race tests + gate

**Files:** Modify: `crates/bridge-coordinator/src/session_manager.rs` (Race-2 manager test),
`crates/bridge-a2a-inbound/src/server.rs` (Race-2 A2A integration if a deterministic hook exists).

- [ ] **Step 1: Race 1 (manager)** — already covered by Task 1's `checkout_mints_unique_op_nonce_per_turn`
(the stale `finish_turn(old_op)` no-ops the new turn). Add a `record_usage`-stale variant.

- [ ] **Step 2: Race 2 (manager + producer)** — a test that force-resets a Running handle whose producer is
gated mid-`backend.prompt`, then asserts: (a) the producer's abort token is cancelled, (b) the producer ends
Canceled (Task 4/6 tests), (c) a subsequent `checkout_existing_turn` on the same context sees the NEW empty
generation (SPEC-FIX-7 — NOT `SessionNotFound`), recall=none.

- [ ] **Step 3: Full gate.** `cargo test -p bridge-coordinator -p bridge-a2a-inbound`;
`cargo test --workspace --no-run`; `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D
warnings`.

- [ ] **Step 4: Commit.** `test(cancel-tokens): Race 1 + Race 2 DoD coverage`

---

## Out of scope (documented; NOT in this plan)
- **F3** `session_clear` strand-window hardening (the `workflow_runs`-lock migration + the clear wire-contract
  decision) — its own follow-up (SPEC-FIX-4 + L3).
- **Abort during `ensure_session`** (post-first-poll) — the biased select covers the real Race-2 window
  (pre-first-poll); a backend-side abort inside `ensure_session` is a narrower follow-up (SPEC-FIX-5).
- **Stale `store.put(task, old_session)`** on force-clear-then-same-task-resend (SPEC-FIX-8) — a gen/op-aware
  store update, separate follow-up.
- **MCP `force` clear** — MCP `clear` stays force-less this slice (SPEC-FIX-3).

## Self-review (done)
- **Spec coverage:** F1 (Tasks 1) ✓; F2 manager (Task 2) ✓; F2 plumbing (Task 3) ✓; F2 producers — streaming
  (4) + unary (5) + Coordinator (6) ✓; DoD (7) ✓. F3 correctly excluded.
- **Type consistency:** `WarmTurn.abort` / `LocalDispatch.abort` are both `CancellationToken`; `turn_abort:
  Option<CancellationToken>` on the handle; the nonce is `OperationId` everywhere; `finish_turn` keys on it
  unchanged.
- **Compile-safety:** the `op`-param removal ripple is contained in Task 1 (signature + ALL callers + tests in
  one task → compiles at the end); later tasks are additive.
