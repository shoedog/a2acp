You are doing a focused CODE REVIEW of ONE just-committed implementation increment of Slice 3 (clear/reset) in
the a2a-bridge Rust workspace (session-cwd = the repo). READ-ONLY: read files, grep, `git`; do NOT
edit/build/test. Be rigorous + decisive. Severity-tag BLOCKER/MAJOR/MINOR.

The increment under review is the MOST RECENT commit. Inspect it: `git show --stat HEAD` then `git show HEAD`.
The task this increment implements (+ the binding plan-fixes it must honor) is:

{{input}}

GROUND TRUTH (BINDING): the plan `docs/superpowers/plans/2026-06-18-slice-3-clear-reset.md` (read the relevant
Task + the **"## v2 — dual plan-review fixes folded"** + **v3/v4** sections — PF-1..PF-15) and the spec
`docs/superpowers/specs/2026-06-18-slice-3-clear-reset.md` (the **"## v2 — dual spec-review fixes folded"**
section, FIX-1..FIX-11). Also read the touched code + neighbors: `crates/bridge-a2a-inbound/src/
{session_manager.rs,server.rs}`, `crates/bridge-core/src/{ports.rs,ids.rs}`, `bin/a2a-bridge/src/main.rs`.

KEY BINDING RULES that may apply to THIS increment (flag any it was supposed to honor but didn't):
- **FIX-3 / generation guard:** `finish_turn`/`record_usage` mutate ONLY if `gen == handle.generation && state
  == Running`; a mismatch is a read-only NO-OP (touch nothing — state/op/last_used/usage). Threaded through
  `WarmTurn`/`WarmTurnGuard`/both producer taps.
- **PF-10:** `cancel` must refresh `last_used` on `Running→Idle` (else a cancelled session reaps early once the
  producer Drop's `finish_turn` no-ops).
- **FIX-2 / claim-before-cancel** (T2): claim `Running→Resetting` atomically, never via Idle/`self.cancel`.
- **FIX-4 / capture-then-EXPIRE** (T2): fallible `configure_session` captured, no early `?`, commit-or-EXPIRE.
- **PF-13:** the `force` pre-cancel is REQUIRED (the API backend's default `release_session` doesn't cancel).
- **FIX-1:** the wire method is `SessionClear` (CamelCase, slash-free), CLI verb `session clear`.
- **DIVERGENCE-1:** clear = NEW bridge SessionId per generation + release old (NOT an in-place reset).

REVIEW:
1. **Correctness** — does the committed code do what the task specifies, RIGHT against the real shapes
   (signatures, enum/match exhaustiveness, lock/await, borrow, error mapping, the concurrency claim model)? Any
   logic bug, race (ABA/TOCTOU), deadlock, or stale-write hole?
2. **Plan/spec faithfulness** — implements the task AND honors the binding FIX-*/PF-* that apply to THIS
   increment? Flag any missed.
3. **No regression** — does it break Slice 0/1/2 behavior or any existing test? (the warm-continue/reconcile/
   release/idle-reap/usage paths; the `finish_turn`/`record_usage` signature ripple; exhaustive matches under
   `--all-targets`.)
4. **Tests** — real (assert the actual contract, not trivially-true) and covering the task's risk? Anything
   untested that should be? (esp. the generation-guard no-op, the cancel-TTL refresh, the Resetting-window
   stale-write keystone, the FIX-4/FIX-7 paths.)
5. **Ambiguity/debt** — anything left as a stub/placeholder or a fragile shape a later task will trip on.

OUTPUT: findings by severity (file:line + concrete fix); then a one-line verdict:
`INCREMENT VERDICT: ship | fix-then-ship`. If `fix-then-ship`, list the EXACT minimal fixes. Be concise.
