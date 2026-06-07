# ADR-0023 — Review→Tweak Loop (Containerized Agents, Slice B2b-3b)

**Date:** 2026-06-06
**Status:** Accepted

**Builds on:** B2b-1 (`implement`, ADR-0019), B2b-2 (verify, ADR-0020), the `:ro` reaper (ADR-0021),
B2b-3a (review-the-diff, ADR-0022). Capstone of the B2b-3 self-correcting loop.

---

## Context

After `implement` commits a change, B2b-2 verifies it and B2b-3a reviews it — but a verify-FAIL or a
review-REJECT just landed in the hand-off for a human to act on. B2b-3b closes the loop: on an actionable
failure, re-prompt the `impl` agent to FIX on the SAME clone, AMEND the fix into the single commit, and
re-verify/re-review — bounded by `[implement].max_attempts`. Advisory (exit 0): it TRIES to reach
APPROVE+PASS; on the bound it hands off the best-effort branch + the final state for the operator to accept.

## Decision

A bounded post-commit loop wraps the `Action::Commit` arm: `verify → review → classify → (fix → amend →
reset-worktree) → repeat`.

- **Two-phase fallibility (load-bearing).** Phase 1 (pre-first-commit) keeps `?`/fail-loud. Phase 2 (after
  the first commit) is lossy: the loop has **NO `?` and no panic** — every fallible op reduces to a
  `StopReason`; the verify/review steps are **total** helpers; the pure `classify`/`build_fix_input`/
  `failure_digest`/`loop_outcome_suffix` cannot panic. The hand-off ALWAYS prints.
- **Injectable loop + a `TweakEffects` seam.** `run_tweak_loop(clone, branch, …, &mut dyn TweakEffects)`
  runs the git ops on a REAL clone while the workflow effects (verify/review/fix) are injected — so the
  no-work-loss wiring is unit-tested with a FAKE executor against a real git repo. Production wires a
  `ProdEffects` impl in `implement_cmd`. (The plan dual-review's keystone catch: an inlined loop had no seam,
  so the spec-mandated no-work-loss test could not be written.)
- **AMEND into one commit, original message pinned.** Each fix `git commit --amend --no-edit` folds into a
  SINGLE commit whose parent stays `base_sha`, KEEPING the attempt-1 message (fix turns do NOT write
  `.git/A2A_COMMIT_MSG`). So `handoff_text` is byte-identical and `cherry-pick -n FETCH_HEAD` stays correct;
  the loop appends report lines at the call site. The hand-off is rebuilt post-loop with the FINAL sha.
- **No silent work loss on agent divergence.** A fix turn that self-commits or switches branches trips the
  HEAD guard → `restore_branch(branch, last_good_sha)` = `checkout -f <branch>` then `reset --hard <sha>`,
  which restores the BRANCH REF the hand-off fetches (a bare `reset --hard` would leave OUR branch at the
  rogue tip if the agent switched branches — the plan dual-review's correctness catch). A failed restore →
  `RestoreFailed` whose suffix marks the branch UNTRUSTED. The `completed==false` case is distinguished from
  a HEAD mutation (`FixIncomplete`, not `HeadMutated`).
- **Verify the COMMITTED tree.** `reset_worktree_to_head` (reset --hard HEAD + clean -fdq) runs before each
  verify so the agent's unstaged scratch can't change the verdict (also closes a latent B2b-2 gap).
- **Pure `classify` (verdicts) + structural stops.** ok = verify {Ran(passed), NotConfigured} and review
  {Approve incl. degraded reviewers_failed>0, NotConfigured}; actionable = verify Ran&&!passed OR review
  Ran{Reject}; anything else (Inconclusive/Incomplete/ConfigError/NotLoaded) → `NotActionable` (re-prompting
  blind thrashes). Order: Success → NotActionable → BoundReached → Continue.
- **Config lifecycle, pre-clone.** `[implement].max_attempts` (default 3, 0→ConfigError, >10 clamp) +
  `fix_workflow` are resolved BEFORE the clone (a malformed block fails loud with no quarantine left); absent
  → `LoopConfig::default()`. A valid-but-unregistered fix workflow → `FixUnavailable` (soft). `implement-fix`
  is **example-only** (the init scaffold has no `impl` agent; init workflow count stays 5).

## Components

| Concern | Home |
|---|---|
| pure `StopReason`/`LoopStep`/`LoopReport`/`FixDisposition`/`classify`/`fix_step`/`build_fix_input`/`loop_outcome_suffix` + injectable `run_tweak_loop` + `TweakEffects` (+ fake-executor tests) | `bin/a2a-bridge/src/tweak.rs` (new) |
| `VerifyOutcome::Incomplete` + pure `failure_digest` | `bin/a2a-bridge/src/verify.rs` |
| `ImplementToml`/`LoopConfig`(+`Default`)/`to_config` + `RegistryConfig.implement` | `bin/a2a-bridge/src/config.rs` |
| `host_commit_argv_run`/`commit_amend_argv`/`host_amend_commit`/`reset_worktree_to_head`/`restore_branch` | `bin/a2a-bridge/src/implement.rs` |
| total `drain_impl`/`run_verify_step`/`run_review_step`; `ProdEffects`; pre-clone `loop_cfg`; the loop call + `LoopFinal` hand-off | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) |
| `implement-fix` workflow + prompt (EXAMPLE-ONLY) + `[implement]` block | `examples/a2a-bridge.containerized.toml` + `prompts/implement-fix.md` |

## Cross-check + dual-review folds

Firewalled clean-room `design` cross-check (commit-strategy → AMEND; dedicated `implement-fix`). Dual
spec-review + dual plan-review (containerized dogfood, leak-safe post-reaper, + a2a-local codex-review)
drove: the silent-work-loss fix (`restore_branch` + `HeadMutated`/`RestoreFailed`), the `TweakEffects` seam
+ fake-executor test (the no-seam BLOCKER), the `Action::Abort` overload → `FixIncomplete`, the pre-clone
config placement, red-first TDD sequencing, and the `classify` cross-product cells. The `matches!`-moves
"BLOCKER" both reviewers raised was **verified a false positive** (it borrows) — the explicit `match &`
form was adopted anyway for clarity.

## Validation

- Unit (Docker-free): `classify` cross-product (ok/actionable/bound × verify {Ran±,NotConfigured,
  ConfigError,Incomplete} × review {Approve±failed,Reject,Inconclusive,NotConfigured,NotLoaded,Incomplete});
  `fix_step`; `build_fix_input`/`failure_digest`/`loop_outcome_suffix`; `ImplementToml::to_config`;
  temp-repo amend (folds to one commit, parent==base, message kept) + `reset_worktree_to_head` +
  `restore_branch` (head-advance AND branch-switch recovery). **Fake-executor `run_tweak_loop` tests:**
  reject→approve amends one commit; **self-commit-after-amend preserves the cumulative A+B tree** (the
  no-work-loss proof); branch-switch divergence restores our branch; no-progress/fix-incomplete/
  fix-unavailable/bound. clippy `-D warnings` clean; fmt clean.
- Live gate (Docker, dogfooded on this repo): **right-first-try** → verify PASS + review APPROVE +
  converged (1 commit, reaper 0-leaked); **verify-fail → fix → PASS** → fmt-fail→fix→clippy-fail→fix→PASS,
  converged at attempt 3, ONE amended commit; **bound + multi-driver** → both verify-FAIL and review-REJECT
  drove `Continue`, amended into one commit, BoundReached at max=3, exit 0; **`max_attempts=1`** → 1 attempt,
  no fix turn, BoundReached, exit 0. Reaper held (0 `:ro`/`:rw` leaked) every run.
- **Live-gate findings (fixed inline):** (1) `parse_verdict` read an *agreeing restated* `VERDICT: APPROVE`
  (lead + footer, both APPROVE) as Inconclusive — now tail-anchors on the LAST VERDICT line and requires all
  VERDICT lines to AGREE (conflict/unknown → Inconclusive). (2) the fix agent reformatted but didn't
  `git add` (the bridge folds only the staged index) — the `implement-fix` prompt now mandates staging
  (unstaged = discarded). **Forcing-a-failure GOTCHA:** a task that ENCODES the bug in its spec creates a
  verify-⊥-acceptance Catch-22 (fixing verify makes the review REJECT on acceptance → the loop oscillates to
  the bound); use an **acceptance-orthogonal** failure (e.g. `clippy::ptr_arg`) to demo a clean converge.
- Coverage (floors per ci.yml): new code is the `a2a-bridge` bin — **tweak.rs 98.36% region / 94.20% line**,
  verify.rs 95.81%, review.rs 99.19% — workspace **89.25% line** (floor 85); the floored library crates
  (bridge-core/acp/api/workflow ≥90) are untouched.

## Deferred (slice-sized only)

A `--gate` flag (exit non-zero on unclean); a per-fix-turn wall-clock timeout (max_attempts bounds turns,
not time); "best-not-last" recovery (tag pre-amend tips); skipping review when verify is already actionable;
a warm cross-turn agent.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
