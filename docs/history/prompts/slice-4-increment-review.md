You are doing a per-increment CODE REVIEW of the latest commit (`git show HEAD`) on the Slice 4 (Compact)
branch of a2a-bridge. READ-ONLY: `git show HEAD`, read files, grep; do NOT edit/build/test. Judge whether the
increment correctly + faithfully implements its task against the frozen spec + plan, and whether it is correct
Rust against the surrounding shipped code. Severity-tag BLOCKER / MAJOR / MINOR with concrete fixes (file:line).

The increment under review (which task + what to focus on):

{{input}}

GROUND TRUTH (cite file:line):
- The commit: run `git show HEAD` (and `git log --oneline -6` for context).
- Spec: `docs/superpowers/specs/2026-06-18-slice-4-compact.md` — the "## v2" FIX-1..14 list is BINDING (esp.
  FIX-1 bad-summary EXPIRES the handle — never restore-Idle; FIX-3 configure-error not SessionExpired; FIX-5
  timeout; FIX-9 cwd parse before the `Compacting` flip).
- Plan: `docs/superpowers/plans/2026-06-18-slice-4-compact.md` — the matching Task's literal code + PFIX list.
- The shipped patterns the increment must match: `crates/bridge-a2a-inbound/src/session_manager.rs`
  `reset_session` (`:434-520`, the claim block + commit/EXPIRE tail), `checkout_turn` non-clean tail
  (`:276-292`), `is_claimed`, `reap_idle` (`Compacting` must be reaper-immune); `crates/bridge-core/src/
  ports.rs` `AgentBackend::prompt`/`Update`; `crates/bridge-core/src/error.rs` (`AgentCrashed`/`MessageTooLarge`
  → `-32603`).

REVIEW CHECKS (ground each in code):
1. **Spec/plan faithfulness:** does the increment implement the named task's steps + the relevant FIX-N exactly?
   Any drift, scope creep, or missed step?
2. **Correctness (the claim state machine, if this increment touches `compact_session`):** is the claim
   `Idle→Compacting` atomic under one lock with the fallible cwd parse BEFORE the flip (no stranded
   `Compacting` on a cwd error — FIX-9)? Is the summarize time-bounded (FIX-5)? Does EVERY post-claim failure
   path EXPIRE (release old once, remove handle, drop lease — no double-release, no restore-Idle — FIX-1)? Is
   `expire_after_summarize` faithful to `checkout_turn:276-292`? Does the good-summary reset tail mirror
   `reset_session:475-519` incl. the revalidate-exact-claim + commit-or-EXPIRE + FIX-3 (configure error, not
   SessionExpired)? Is `pending_seed` stashed only on commit? Any `?` between the claim and a resolved state
   that could strand `Compacting`?
3. **Rust correctness:** does it compile as written (types, the `compact_session<F,Fut>` closure bounds, no
   guard held across `.await`, `Send`)? Any borrow/lock-ordering hazard?
4. **Tests:** do the increment's tests actually assert the behavior (e.g. EXPIRE = `status()==None` + old
   released; advance = generation N+1 + `pending_seed` Some)? Any test that passes vacuously or asserts the
   wrong thing?
5. **No regressions:** does it preserve the shipped `reset_session`/`checkout_turn`/reconcile invariants?

OUTPUT: findings by severity (file:line, concrete fix). End with exactly:
`INCREMENT VERDICT: clean | fix-required`.
