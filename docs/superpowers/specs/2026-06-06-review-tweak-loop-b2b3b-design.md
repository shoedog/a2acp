# B2b-3b — Review→Tweak Loop — Design

**Date:** 2026-06-06
**Status:** Draft (pre dual-review).
**Builds on:** B2b-1 (`implement`, ADR-0019), B2b-2 (verify, ADR-0020), the `:ro` reaper (ADR-0021),
B2b-3a (review-the-diff, ADR-0022). This is the capstone of the B2b-3 self-correcting loop.

## Goal

Make `implement` self-correcting: when the committed change FAILS verify or is REJECTED by review,
re-prompt the impl agent on the SAME persistent clone (which has the prior commit) to FIX the issues, then
re-commit / re-verify / re-review — bounded by a max-iterations cap. The loop wraps the existing linear
`[edit → commit → verify → review]`. ADVISORY: it TRIES to reach APPROVE+PASS; on cap-exhausted it hands
off the best-effort branch + the final state (the operator accepts at merge).

## Decisions (settled with the owner)

1. **The loop wraps `[edit → commit → verify → review]`.** One-time setup (clone, registry+executor,
   `clone_cwd`, `base_sha`, `verify_cfg`/`review_cfg`, the graphs) stays OUTSIDE; per-iteration: re-snapshot
   HEAD (for `head_guard`), strip `.git/A2A_COMMIT_MSG`, build the iteration input, run a fresh `:rw` edit
   turn (reaped), `decide()`/commit, verify, review.
2. **Tweak trigger = clear `verify-FAIL` OR `review-REJECT`.** Continue iff `verify == Ran{passed:false}`
   OR `review == Ran{Reject}`. STOP (hand off) on: APPROVE+PASS (success); review `Inconclusive` / review
   `Incomplete` (timeout) / verify `Incomplete` / `NotConfigured` (not actionable — surfaced in the hand-off
   for the human, NOT churned on); the iteration cap; or a non-Commit `decide()` Action (see #6).
3. **`[implement].max_iterations` (default 3 = the initial attempt + up to 2 tweaks).** New config block;
   absent → 3. **ADVISORY**: cap-exhausted → hand off the best-effort branch + the final verify/review
   state; `implement` exits 0. (A `--gate` / exit-non-zero-on-unclean flag is a deferred follow-on slice.)
4. **Checkpoint chain + range-squash merge.** Each iteration is a NEW commit on the task branch
   (`base → C1 → C2 → …`), preserving the tweak history in the clone (debuggable); verify + review always
   see the CUMULATIVE `base_sha..HEAD`. The hand-off merge command becomes `cherry-pick -n
   <base_sha>..FETCH_HEAD` (the RANGE) so the operator gets ONE squashed commit of the cumulative change.
   (The range form is also correct for a single commit — `base..C1` = C1 — so no special-casing.)
5. **Tweak context = the prior verify-failures + review-findings, augmenting the SAME `implement-edit`
   workflow** (no new workflow). The bridge captures the failing verify command output + the review synth
   body and builds an augmented input: the original task + "your previous attempt had these issues — FIX
   them on the current clone (it has your prior commit): <verify failures> <review findings>". The
   `implement-edit` contract (stage + write `.git/A2A_COMMIT_MSG`, don't commit) is identical for a tweak.
6. **`decide()` Actions in the loop.** `Commit` → commit + continue the verify/review. A NON-Commit Action
   ends the loop: iteration 1 `NoCommitClean`/`NoCommitDirty` keep the existing "made no change / staged
   nothing" hand-off; a TWEAK iteration that produces `NoCommitClean`/`NoCommitDirty` = **no progress** →
   stop + hand off the last committed state (don't loop forever on a stuck agent); `Abort` → stop + the
   existing abort path (leave the clone).
7. **The B2b-2/B2b-3a invariants carry per-iteration:** the post-commit tail stays infallible (precomputed
   `clone_cwd`; best-effort post-commit stage; `parse_verdict` can't panic); each review is bounded by its
   timeout; the `:rw` edit container + the `:ro` review containers are reaped each iteration.

## Architecture

### Pure loop-decision (the coverage keystone; `implement.rs` or `review.rs`)
```rust
/// What to do after an iteration's verify+review. PURE — the loop's riskiest branching, unit-tested as a
/// matrix (mirrors B2b-1 `decide()` / B2b-2/3a `outcome_suffix`).
pub enum LoopStep { Tweak, Stop(StopReason) }
pub enum StopReason { Clean, NotActionable, CapReached }  // Clean = PASS+APPROVE; NotActionable = Inconclusive/Incomplete/NotConfigured
pub fn loop_step(
    verify: &verify::VerifyOutcome,
    review: &review::ReviewOutcome,
    iteration: usize,
    max_iterations: usize,
) -> LoopStep;
// Tweak  iff (verify is Ran{passed:false} OR review is Ran{Reject}) AND iteration < max_iterations
// Stop(Clean) iff verify passes(or not-configured) AND review Approve(or not-configured)
// Stop(CapReached) iff would-tweak but iteration == max_iterations
// Stop(NotActionable) otherwise (Inconclusive / Incomplete / a verify infra error with no actionable fail)
```

### Pure tweak-input + context capture (`review.rs`/`verify.rs`/`implement.rs`)
```rust
// verify.rs — PURE: the failing-command block for a tweak (name + truncated output of each !ok gate result).
pub fn failure_digest(verdict: &VerifyVerdict, max_bytes: usize) -> String;
// review.rs — already dumps the synth body; capture it (the full synth text from the Ran path) as findings.
// implement.rs (or review.rs) — PURE: the augmented edit input for a tweak.
pub fn build_tweak_input(task: &str, verify_failures: &str, review_findings: &str) -> String;
```
The first iteration uses `a.task`; tweaks use `build_tweak_input(task, <verify failure_digest>, <review synth body>)`.

### `[implement]` config (`config.rs`)
```rust
pub struct ImplementToml { #[serde(default)] pub max_iterations: Option<usize> }
pub struct ImplementConfig { pub max_iterations: usize }   // absent/0 → 3
// RegistryConfig:  #[serde(default)] pub implement: Option<ImplementToml>
```

### The loop in `implement_cmd` (`main.rs`)
- One-time setup unchanged (clone, registry/executor, `clone_cwd`, `base_sha`, configs, graphs, the
  `implement-review` graph lookup hoisted out of the per-iteration arm).
- `let mut pre = pre_initial_head;` `let mut iteration = 0;` `let (mut verify_out, mut review_out) = (NotConfigured, NotConfigured);` plus `let (mut verify_failures, mut review_findings) = (String, String)`.
- LOOP:
  1. `iteration += 1;` strip `.git/A2A_COMMIT_MSG`; snapshot `pre = head_sha(&clone)`.
  2. input = iteration 1 ? `a.task` : `build_tweak_input(&a.task, &verify_failures, &review_findings)`.
  3. run `implement-edit` (graph.clone(), input, run_id=`impl-<task>-iter<N>`, fresh cancel, ctx{clone_cwd}); drain → `completed`.
  4. `decide(completed, head_guard(&clone,&branch,&pre), stage_state, commit_message)`:
     - non-`Commit` → handle (iter-1 paths / tweak-no-progress / abort) and `break`.
     - `Commit(msg)` → `sha = host_commit`; strip `A2A_COMMIT_MSG`.
  5. verify (B2b-2) → `verify_out`; if `Ran` and not passed, `verify_failures = failure_digest(...)`.
  6. review (B2b-3a, on `base_sha..sha`) → `review_out`; capture the synth body into `review_findings` when not Approve.
  7. `match loop_step(&verify_out, &review_out, iteration, max) { Tweak => continue, Stop(r) => break }`.
- Hand-off (after the loop): `handoff_text` extended to report the iteration count + the final verify/review
  state + the StopReason, and the **range-squash merge** command (`cherry-pick -n <base_sha>..FETCH_HEAD`).

### Hand-off (`implement.rs::handoff_text`)
Extend (or a new sibling) to take `iteration`, `max`, the final `verify`/`review` suffixes, and the
`StopReason`; emit e.g.:
```
implement: <iterations> iteration(s); committed <tip-sha> on <branch>  [<Clean|cap reached|needs human>]
verify: PASS (…)        review: APPROVE (…)
clone: <path>
To merge as YOURSELF (squashes the tweak chain) and reap the clone:
   git -C "<repo>" fetch "<clone>" <branch>
   git -C "<repo>" cherry-pick -n <base_sha>..FETCH_HEAD && git -C "<repo>" commit --reset-author
   rm -rf "<clone>"
```

## Component / file boundaries

| Concern | Home |
|---|---|
| pure `loop_step`/`StopReason`, `build_tweak_input` | `bin/a2a-bridge/src/review.rs` (or implement.rs) |
| pure `failure_digest` (verify failing-command block) | `bin/a2a-bridge/src/verify.rs` |
| `ImplementToml`/`ImplementConfig` + `RegistryConfig.implement` | `bin/a2a-bridge/src/config.rs` |
| the loop + per-iteration HEAD snapshot + context capture; extended hand-off | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) + `implement.rs::handoff_text` |

## Testing
- **Unit (no Docker):** `loop_step` matrix — FAIL→Tweak, REJECT→Tweak, PASS+APPROVE→Stop(Clean),
  Inconclusive/Incomplete→Stop(NotActionable), would-tweak-at-cap→Stop(CapReached), NotConfigured verify/
  review→don't block; `failure_digest` (only !ok gates, truncated); `build_tweak_input` (task + both
  blocks); `handoff_text` (iteration count + range merge + final state); `ImplementToml::to_config`
  (absent/0→3, explicit). The loop wiring itself is impure (live-gated); the decision + input-building are
  pure-tested.
- **Live gate (Docker, dogfooded):** (1) a task the agent gets right first try → 1 iteration, APPROVE+PASS,
  one checkpoint, range-merge command. (2) a task that FAILS verify on the first attempt (e.g. introduces a
  clippy/test break) → the tweak gets the failure digest, fixes it, iteration 2 PASS+APPROVE; assert ≥2
  checkpoint commits + `cherry-pick -n base..FETCH_HEAD` applies the cumulative change. (3) cap-exhausted
  (`max_iterations=1` + a hard task) → hand off best-effort + `[cap reached]` + exit 0. (4) the `:rw` edit
  + `:ro` review containers reaped each iteration (poll-to-0). (5) the commit/hand-off ALWAYS print.

## Deferred (only slice-sized items; smaller suggestions folded inline per owner)
- A `--gate` / exit-non-zero-on-unclean flag (CI/automation) — its own small slice.
- Warm-pool reuse (keep the SAME warm agent+container across tweaks instead of a fresh per-turn container) —
  a separate slice (the warm-pool work). B2b-3b uses the persistent CLONE as the continuity, not a warm agent.
- Per-iteration timeout tapering; resumable/crash-resume of an in-flight loop.

## Firewall
Designed from the bridge's own seams (the `implement_cmd` loop boundary, `decide()`/`head_guard`,
`verify`/`review` outcomes). Owner cadence: a firewalled clean-room `design`-workflow cross-check (run
separate from this spec) THEN the dual spec-review (containerized dogfood + a2a-local codex). Capstone of the
self-correcting coding loop.
