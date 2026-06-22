# Warm-turn cancellation tokens — HANDOFF / Resume Doc

> Single entry-point to resume the **Slice-9 prerequisite** (warm-turn cancellation tokens). Written
> 2026-06-21 as compaction insurance mid-implementation. Read top-to-bottom, then
> `git log --oneline main..HEAD` on `feat/warm-turn-cancellation-tokens` to see where it stopped.

## Where this sits in the roadmap
- **Slice 8 (MCP/D1 surface): SHIPPED + MERGED + PUSHED** (`origin/main` `52b4f9a`; HANDOFF update `53ee12f`).
  See [[slice-8-mcp-shipped]] + `docs/superpowers/2026-06-21-slice-8-T8-T10-HANDOFF.md`.
- **Analysis (done):** no debt blocks the prereq or Slice 9. T9 deep-unification should come AFTER (not before)
  a race-free turn foundation. F3 (`session_clear` spawn-detach) is a bundle candidate but was SPLIT OUT (too
  tangled — `workflow_runs`-lock migration + wire-contract change). Everything else (Finding-3 clock seam, A2A
  unary truncation, detached usage, cap-gated actions) is independent.
- **THIS work = the gating prereq for Slice 9** (Turn Channel + E2 permission — a cancel-under-concurrency
  feature). After this lands → Slice 9, then the Slice-10+ tail (B2 fan-out panel · E1 worktree · E6 retry ·
  E3 batch · E7 task-spec · E8 prompt-lib).

## State (2026-06-21) — branch `feat/warm-turn-cancellation-tokens` (NOT pushed, NOT merged)
- Base = `main` = `origin/main` `52b4f9a`.
- **Commits so far (docs only):** `b4b40cf` spec v2 · `a48d0b2` plan · `8b4cf84` plan v2. NO code committed yet.
- **IN FLIGHT:** codex-HIGH implementing **Task 1** via `run-workflow` (background; out → `/tmp/cancel-tokens-task-1.out`).
  On resume: check if it finished (read that file + `git status`), then VERIFY in the clean host env + commit
  (see the loop below). If it didn't finish / the tree is dirty-but-uncommitted, re-verify the post-state.

## Binding docs (the rulings are SETTLED — do NOT re-litigate)
- **Spec:** `docs/superpowers/specs/2026-06-21-warm-turn-cancellation-tokens.md` — the **`## v2 (SPEC-FIX-1..8 +
  L1..L3)` section is BINDING** and supersedes the draft.
- **Plan:** `docs/superpowers/plans/2026-06-21-warm-turn-cancellation-tokens.md` — the **`## v2 (PLAN-FIX-1..7)`
  section is BINDING** and supersedes contradicting task-body text. 7 TDD tasks.

## What it does (F1 + F2; F3 OUT)
- **F1 — Race 1 (cancel→next-turn op collision):** `SessionManager` mints a UNIQUE op nonce (`turn-{n}`,
  `AtomicU64`) per checkout (drop the caller `op` param); `finish_turn`/`record_usage`'s gen+op guard then
  discriminates turns even without a generation bump. (Slice 8 already closed this on MCP via the Coordinator's
  own nonce; this UNIFIES both surfaces.)
- **F2 — Race 2 (`clear --force` vs producer-start → context resurrection):** a per-turn `CancellationToken` on
  the handle → `WarmTurn` → `LocalDispatch`; producers race it in a **`biased;` select (abort arm FIRST)**
  before/while polling the translator (pre-first-poll abort prevents the `backend.prompt` re-mint — the real
  Race-2 window is the `store.put` gap); `reset_session(force)` cancels it under the `Resetting` claim.

## The 7 tasks (with the PLAN-FIX corrections folded — READ plan v2)
1. **Manager mint (IN FLIGHT):** `turn_op_seq` counter + `mint_turn_op()`; `WarmHandle.turn_abort:
   Option<CancellationToken>` (init None everywhere); `WarmTurn.abort: CancellationToken`; mint at ALL 4
   `WarmTurn` sites (`checkout_turn_inner` no-diff-reuse `:274` + clean-reconcile `:409` + fresh-mint `:472`,
   PLUS `checkout_existing_turn` `:254`); **drop the `op` param** from all 4 checkout fns; delete the A2A
   `op-{task}` (`server.rs:770`/`:2273`) + the child dispatcher's derivation; **finish_turn + cancel_inner
   clear/cancel the token (PLAN-FIX-1 BLOCKER)**; the **~100-site op-param sweep** (`rg
   "\.checkout_turn\(|\.checkout_existing_turn\(|\.checkout_child_turn\("` → switch post-checkout guards to
   `&turn.op`); test `checkout_mints_unique_op_nonce_per_turn` (helper is `manager()` NOT `test_manager()`).
2. **Force-reset cancels the token:** in `reset_session_inner`'s claim block (`:814-855`), `let turn_abort =
   h.turn_abort.take();` → after the lock, before `release_session` (`:862`), `if let Some(t)=&turn_abort
   {t.cancel();}`. Test `force_reset_cancels_the_inflight_turn_abort`.
3. **`LocalDispatch.abort`:** add the field (`dispatch.rs`); populate from `turn.abort` in `warm_local_dispatch`
   (`server.rs:512`); add `CancellationToken::new()` to BOTH `resolve_configure_bind` constructors (`:440` +
   `:477`, PLAN-FIX-3).
4. **Streaming producer biased abort-select:** `spawn_local_producer` (`:1390`) — make the existing select
   (`:1416`) `biased;` with the abort arm FIRST (emit `Event::terminal(TaskOutcome::Canceled)` via `tx`, return).
   **PLAN-FIX-5 test:** a backend that flags if `prompt` is EVER called + a PRE-cancelled token → assert Canceled
   terminal AND `prompt_called == false` (the no-re-mint proof).
5. **Unary Local biased abort-select:** convert the `while let` (`:2306`) to a biased select; the existing
   terminal mapping (`:2493`) yields `TASK_STATE_CANCELED`.
6. **Coordinator `collect_turn` biased abort-select:** biased select; **keep `drop(events)` BEFORE finish_turn**
   (PLAN-FIX-6); on abort push `Event::terminal(TaskOutcome::Canceled)` so `stop_reason == "cancelled"`.
7. **DoD:** Race 1 (Task 1 test) + Race 2 (force-reset Running handle whose producer is gated; assert token
   cancelled + Canceled + post-clear `continue` sees the NEW empty generation — NOT `SessionNotFound`,
   SPEC-FIX-7) + full gate (`-p bridge-coordinator -p bridge-a2a-inbound`; `--workspace --no-run`; fmt; clippy).

## The proven implementation loop (USE THIS)
- **Roles:** codex gpt-5.5 HIGH implements (write, NO commit / NO git-mutating cmds); codex gpt-5.5 XHIGH
  reviews (read-only); Opus (controller) architects/controls/verifies+commits/live-gates.
- **Per task:** write `/tmp/cancel-tokens-task-N.md` (grounded in plan v2 + the real APIs), then:
  ```
  ./target/debug/a2a-bridge run-workflow cancel-tokens-impl \
    --input /tmp/cancel-tokens-task-N.md --session-cwd /Users/wesleyjinks/code/a2a-bridge \
    --config examples/a2a-bridge.cancel-tokens-impl-codex.toml --out /tmp/cancel-tokens-task-N.out
  ```
  (background it; edits the tree, does NOT commit). Prompt = `prompts/cancel-tokens-impl.md` (carries the role
  rules + binding-doc pointers).
- **Then (controller):** VERIFY in the clean host env (codex's sandbox hits the `_dyld_start`/rustc-stall flake
  and often can't run tests — its `DONE_WITH_CONCERNS` is usually just the flake). Re-run `cargo test -p
  bridge-coordinator -p bridge-a2a-inbound` + `--no-run` + fmt + clippy yourself. Stage ONLY the task's files
  (the worktree has MANY unrelated untracked `examples/*.toml`/`prompts/*.md` — do NOT fold them; also the
  pre-existing `M examples/a2a-bridge.slicing-analysis.toml` is NOT ours). Commit.

## After all 7 tasks
- Whole-branch dual-lens review (codex-xhigh via the bridge — mirror the slice-8 whole-branch scaffolding + a
  new prompt; ports used so far: spec-review 8118, plan-review 8119, impl 8120) + an Opus lens on `git diff
  main...HEAD`. Pressure-test: the re-mint guarantee (biased select), the token-clearing on EVERY turn-end path,
  no stranded tokens, the gen+nonce guard, byte-identity elsewhere.
- **Live-gate vs real codex:** Race 1 (cancel-then-immediate-resend doesn't clobber the new turn) + Race 2
  (force-clear during a turn leaves the context at a new empty generation, not resurrected) on the A2A surface
  (MCP has no `force` — SPEC-FIX-3). Build the binary; a scripted client (the slice-8 NDJSON driver pattern or
  the A2A wire).
- Merge `--no-ff` to `main` once clean; update memory (`slice-3-clear-reset-shipped` deferred-hardening block
  is now CLOSED for F1/F2; write a `cancel-tokens-shipped` note) + the orchestration HANDOFF; push.

## OUT OF SCOPE (documented; separate follow-ups)
- **F3** `session_clear` strand-window hardening (`workflow_runs`-lock migration + the clear wire-contract
  decision) — its own follow-up.
- **Abort during `ensure_session`** (post-first-poll) — the biased select covers the real Race-2 window
  (pre-first-poll); a backend-side abort is a narrower follow-up (SPEC-FIX-5).
- **Stale `store.put(task, old_session)`** on force-clear-then-same-task-resend (SPEC-FIX-8) — gen/op-aware
  store update, separate.
- **MCP `force` clear** (SPEC-FIX-3) — MCP `clear` stays force-less this slice.

## Scaffolding on the branch (uncommitted unless noted)
- Spec/plan: committed (`b4b40cf`/`a48d0b2`/`8b4cf84`).
- Review/impl tooling (committed with their phases): `prompts/cancel-tokens-{spec-review,plan-review,impl}.md`
  + `examples/a2a-bridge.cancel-tokens-{spec-review,plan-review,impl}-codex.toml`. The codex review OUTPUTS are
  in `/tmp/cancel-tokens-{spec,plan}-review.out` (transient).
