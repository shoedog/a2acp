# B2b-3b — Review→Tweak Loop — Design

**Date:** 2026-06-06
**Status:** Draft (rev3 — folds the dual spec-review: containerized dogfood PRIMARY + a2a-local codex
backstop, both needs-changes; spine + amend confirmed). rev2 folded the clean-room cross-check + owner
decisions (AMEND, dedicated `implement-fix`).
**Builds on:** B2b-1 (ADR-0019), B2b-2 (ADR-0020), the `:ro` reaper (ADR-0021), B2b-3a (ADR-0022). Capstone
of the B2b-3 self-correcting loop.

## Goal

Make `implement` self-correcting: on verify-FAIL or review-REJECT, re-prompt the impl agent on the SAME
persistent clone to FIX, then re-commit (git AMEND) / re-verify / re-review — bounded by `max_attempts`.
ADVISORY: it TRIES to reach APPROVE+PASS; on the bound it hands off the best-effort branch + the final
state (the operator accepts at merge).

## Decisions (owner + dual-review fold)

1. **Bounded post-commit control loop** wrapping today's `Action::Commit` arm. One-time setup (clone,
   checkout base, registry/executor, `clone_cwd`, `base_sha`, configs, the graphs) OUTSIDE; the loop:
   `verify → review → classify → (fix turn → amend → reset-worktree) → repeat`.
2. **Strict two-phase fallibility (load-bearing).** Phase 1 (pre-first-commit) keeps the existing `?`/
   fail-loud. Phase 2 (after the first `host_commit`) is **lossy**: the loop body has **NO `?` and no
   panic** — every fallible op is reduced to a terminal outcome and converted to a hand-off. Concretely:
   `head_sha`/`stage_state`/`host_amend_commit`/`reset_worktree`/`reset_hard` are `match`ed → a `StopReason`;
   `run_verify_step`/`run_review_step` are **total helpers** (return an Outcome, never `?`, reducing stream/
   runtime/timeout to `Incomplete`); `classify`/`build_fix_input`/`failure_digest`/`loop_outcome_suffix`
   cannot panic (byte-wise, no slicing/unwrap — per B2b-3a's em-dash lesson).
3. **AMEND into one commit, original message pinned.** Each fix `git commit --amend`s into a SINGLE commit
   whose parent stays `base_sha`, KEEPING the original (attempt-1) commit message (`--amend` reusing the
   stored message; fix turns do NOT write `.git/A2A_COMMIT_MSG` — the message describes the task = the whole
   change). So `implement::handoff_text()` output is **byte-identical** (its `cherry-pick -n FETCH_HEAD`
   stays correct — the tip is always `base..tip` = the cumulative change); the loop appends report lines at
   the CALL SITE only. verify runs on the committed tree (see #4); review runs `base_sha..HEAD`.
4. **Verify the COMMITTED tree, not the dirty working tree.** `stage_state` calls mixed staged+unstaged
   `Staged`, and verify runs `cd <clone>` against the working tree — so unstaged scratch could change the
   verdict vs the commit. After each commit/amend, **reset the working tree to HEAD** (`git -c …
   checkout -- . && git clean -fdq`, scoped to the clone) so verify tests EXACTLY the committed change.
   (Review's `git diff base..HEAD` is already commit-based; fixes a latent B2b-2 issue.)
5. **`[implement].max_attempts` = MAX COMMIT TURNS** (default 3 = the initial commit + up to 2 fix amends;
   `max=1` ⇒ exactly today's behavior). Under amend the branch always has ONE commit; the value bounds
   commit *operations/turns*, not branch length, and bounds *turns* not wall-clock (a hung fix turn is not
   time-bounded — per-fix timeout deferred). **Full config lifecycle:** absent `[implement]` →
   `LoopConfig::default()` (loop ON, max_attempts=3) — NOT disabled; `max_attempts=0` → ConfigError
   (fail-loud); `>HARD_MAX(10)` → clamp(10)+`eprintln`; `fix_workflow` absent → `"implement-fix"`, malformed
   → ConfigError (pre-commit). ADVISORY (exit 0; `--gate` deferred).
6. **Dedicated `implement-fix` workflow + prompt** [owner]. A new 1-node `impl`-agent workflow framed
   "CONTINUE & fix these specific verify failures / review findings on the current clone; re-stage; DON'T
   commit; DON'T write a commit message (the bridge keeps the original)." Registered **only in the
   containerized example** (NOT `INIT_WORKFLOWS` — the init scaffold has no `impl`/`container_rw` agent and
   `load_workflows` hard-fails unknown-agent workflows; `implement-edit` is example-only for the same
   reason → the init workflow count stays 5). If `fix_workflow` is absent from a loaded `wf_map` →
   **`FixUnavailable`** degradation (no fix turns; hand off — reachable via a config without it).
7. **Pure `classify` (verdicts only) + structural reasons (orchestration).** Trigger/matrix (verified):
   `verify_ok = Ran(passed)|NotConfigured`; `review_ok = Ran{Approve}|NotConfigured` INCLUDING a degraded
   `Approve{reviewers_failed>0}` (still ok — advisory, per B2b-3a). Actionable: `verify Ran && !passed`;
   `review Ran{Reject}` (any `reviewers_failed`). `ConfigError`/`NotLoaded`/`Incomplete`/`Inconclusive`/
   timeout = neither ok nor actionable → `NotActionable` (fail-safe: re-prompting with no concrete failure
   list is blind thrash; the per-step suffix carries the why). Order: Success → NotActionable →
   BoundReached → Continue.
8. **Fix context (attempts 2+)** = a concrete `build_fix_input` template (below) of the verify
   failure-digest + (only when `review Ran{Reject}`) the **hoisted synth body** (B2b-3a discards it — only
   `summary` is stored — so it must be hoisted loop-scoped), bounded.

## Architecture

### Pure decision core — `bin/a2a-bridge/src/tweak.rs` (NEW)
```rust
pub enum StopReason {
    Success, BoundReached, NotActionable,
    NoProgress,          // a fix turn's decide() = NoCommitClean/Dirty (agent staged nothing new)
    HeadMutated,         // decide() = Abort (head_guard: agent self-committed/switched branch THIS turn)
    AmendFailed, StepError(String), FixUnavailable,
}
pub enum LoopStep { Continue, Stop(StopReason) }
pub struct LoopReport { pub attempts: u32, pub stop_reason: StopReason }
pub fn classify(attempt: u32, max_attempts: u32,
                v: &verify::VerifyOutcome, r: &review::ReviewOutcome) -> LoopStep;  // PURE; cross-product matrix
pub fn build_fix_input(task: &str, verify_digest: &str, review_findings: Option<&str>, max_bytes: usize) -> String;
pub fn loop_outcome_suffix(rep: &LoopReport) -> String;
```
**`build_fix_input` template (fixed order; budget split):**
```
<task>

The previous attempt did not pass. FIX these on the current clone (it has your prior commit); re-stage; do
NOT commit and do NOT write a commit message.

## Verify failures
<verify_digest>            (omitted if empty)

## Review findings (REJECTED)
<review_findings>          (omitted when None — i.e. only on REJECT)
```
Truncation priority: keep the task in full; split the remaining `max_bytes` between the two blocks
(verify first), each via `verify::truncate_output` (head+tail).

### `failure_digest` — `verify.rs` (PURE)
`pub fn failure_digest(verdict: &VerifyVerdict, max_bytes: usize) -> String` — ONLY the `!ok` GATE results,
in order, each `"### <name>\n<truncated output>"` (results carry only `name`/`gate`/`ok`/`output` — no
command/status; empty output → `"### <name>\n(no output)"`); per-result byte budget = `max_bytes /
n_failed`. Empty (no failed gates) → `""`.

### `[implement]` config — `config.rs`
`ImplementToml { max_attempts: Option<u32>, fix_workflow: Option<String> }` → `LoopConfig { max_attempts:
u32, fix_workflow: WorkflowId }`; `impl Default for LoopConfig { max_attempts: 3, fix_workflow:
WorkflowId::parse("implement-fix") }`. `RegistryConfig.implement: Option<ImplementToml>`; the bin maps
`None => LoopConfig::default()`, `Some(t) => t.to_config()?` (pre-commit; 0→ConfigError, >10→clamp).

### `implement.rs` — share the lock-retry + amend (preserve ALL host_commit behavior)
```rust
fn host_commit_argv_run(clone, argv) -> Result<String,String>;  // the 5×index.lock retry + stale-clear + sha read
pub fn host_commit(clone, msg) -> ...        // commit_argv (-c safe.directory/hooksPath/gpgsign/user + --no-verify -m) → run
pub fn commit_amend_argv(clone) -> ...       // SAME -c pins + --no-verify + --amend --no-edit (keeps the stored message)
pub fn host_amend_commit(clone) -> ...       // commit_amend_argv → host_commit_argv_run
pub fn reset_worktree_to_head(clone) -> Result<(),String>;   // -c safe.directory checkout -- . ; clean -fdq
pub fn reset_hard(clone, sha) -> Result<(),String>;          // -c safe.directory reset --hard <sha> (restore trusted tip)
```

### The loop — `main.rs` `implement_cmd`
- Extract total helpers: `async fn drain_impl(stream)->bool` (shared by iter-1 + fix); `async fn
  run_verify_step(...)->VerifyOutcome` and `async fn run_review_step(...)->(ReviewOutcome,String/*synth*/)`
  — both **total** (no `?`), `run_review_step` creating a FRESH `CancellationToken` per call + the
  `select!` timeout→cancel→keep-drain (preserve the reaper).
- After the first `host_commit` → `sha` (the only post-commit `?`); `let original_message = message;`
  `attempt = 1`; **initialize `last_verify = VerifyOutcome::Incomplete` and `last_review =
  (ReviewOutcome::Incomplete, String::new())`** so the always-print hand-off has defined values even if the
  very first `reset_worktree_to_head` fails before any step runs; loop:
  ```
  reset_worktree_to_head(&clone) → on Err stamp StepError, break;
  last_verify = run_verify_step(... sha ...);
  (last_review, synth) = run_review_step(base_sha, sha, attempt);   // run_id impl-review-{task}-{attempt}
  match classify(attempt, max, &last_verify, &last_review) {
    Stop(r) => { report = {attempt, r}; break }
    Continue => {
      let Some(fix_graph) = fix_graph else { report={attempt,FixUnavailable}; break };
      pre_i = head_sha(&clone)?→StepError;
      findings = matches!(last_review, Ran{Reject,..}).then(|| synth.as_str());
      input = build_fix_input(&task, &failure_digest(...), findings, BUDGET);
      completed = drain_impl(run_with_context(fix_graph.clone(), input, "impl-fix-{task}-{attempt}", fresh-token, ctx{clone_cwd}));
      match decide(completed, head_guard(&clone,&branch,&pre_i), stage_state?→StepError, (original_message,false)) {
        Action::Commit(_) => match host_amend_commit(&clone) { Ok(s)=>{sha=s; attempt+=1; continue} Err(_)=>{report={attempt,AmendFailed}; break} }
        Action::Abort(_)  => { let _=reset_hard(&clone,&sha); report={attempt,HeadMutated}; break }   // restore trusted tip
        _                 => { report={attempt,NoProgress}; break }   // NoCommitClean/Dirty
      }
    }
  }
  // hand-off: PATCH the committed line with the FINAL sha + the ORIGINAL subject, then append suffixes.
  handoff = handoff_text(&clone, &branch, &sha, &original_subject, &repo);   // sha = final tip
  handoff += verify::outcome_suffix(&last_verify) + review::outcome_suffix(&last_review) + tweak::loop_outcome_suffix(&report);
  println!(handoff); Ok(())
  ```
- decide() on a fix turn passes the ORIGINAL message ONLY to satisfy its `Staged → Commit(msg)` arm; the
  amend path (`--amend --no-edit`) IGNORES it and reuses the stored attempt-1 message, so the committed
  subject is stable across all attempts (no per-fix `A2A_COMMIT_MSG`; resolves the stale-subject + narrow-
  message findings together). `head_guard` re-snapshots `pre_i` each turn so an agent self-commit THIS turn
  → `Abort` → `HeadMutated` (reset_hard to `sha`, the trusted tip) — NOT folded into `NoProgress`, and the
  hand-off then reflects the trusted `sha`.

## Component / file boundaries

| Concern | Home |
|---|---|
| pure `classify`/`StopReason`/`LoopReport`/`build_fix_input`/`loop_outcome_suffix` | `bin/a2a-bridge/src/tweak.rs` (NEW) |
| pure `failure_digest` | `bin/a2a-bridge/src/verify.rs` |
| `ImplementToml`/`LoopConfig`(+`Default`) + `RegistryConfig.implement` | `bin/a2a-bridge/src/config.rs` |
| `host_commit_argv_run`/`commit_amend_argv`/`host_amend_commit`/`reset_worktree_to_head`/`reset_hard` | `bin/a2a-bridge/src/implement.rs` |
| `prompts/implement-fix.md` + `implement-fix` workflow (EXAMPLE only) | `prompts/` + `examples/a2a-bridge.containerized.toml` |
| `drain_impl` + total `run_verify_step`/`run_review_step` + the loop + final-sha hand-off patch | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) |

## Testing
- **Unit (no Docker):** `classify` **cross-product** — verify {Ran-pass, Ran-fail, NotConfigured,
  ConfigError, Incomplete} × review {Approve+failed=0, Approve+failed>0, Reject+failed=0, Reject+failed>0,
  Inconclusive, NotConfigured, NotLoaded, Incomplete} × {attempt<max, attempt==max} → the right
  LoopStep/StopReason; `build_fix_input` (task-kept-full, verify-only vs review-only sections, ordering,
  bounding); `failure_digest` (only !ok gates, per-result budget, empty-output, no-failures→""); 
  `loop_outcome_suffix` (each StopReason incl HeadMutated); `ImplementToml::to_config` (absent→default,
  0→ConfigError, >10→clamp, fix_workflow default/malformed); `commit_amend_argv` (pins + `--amend
  --no-edit`); temp-repo: amend folds to ONE commit whose parent is still `base_sha` + keeps the original
  message; `reset_hard`/`reset_worktree_to_head` behavior.
- **Live gate (Docker, dogfooded):** (1) right-first-try → 1 attempt, APPROVE+PASS, ONE commit, unchanged
  merge command. (2) a verify-FAIL on attempt 1 (introduce a clippy/test break) → the fix turn gets the
  digest, fixes it, attempt 2 PASS+APPROVE, still ONE amended commit; `cherry-pick -n FETCH_HEAD` applies
  the cumulative change. (3) `max_attempts=1` + a failing task → `loop: 1 attempt — bound reached` +
  best-effort + exit 0. (4) the `:rw` fix + `:ro` review containers reaped each attempt. (5) the commit +
  hand-off ALWAYS print; the printed `committed <sha>` is the FINAL tip. Temp-repo fake-executor
  integration: reject-then-approve, no-progress, amend-fails-mid-loop, **agent-mutates-HEAD →
  reset_hard + HeadMutated (no work loss)**.

## Deferred (only slice-sized; smaller fixes folded inline per owner)
- A `--gate` / exit-non-zero-on-unclean flag.
- A per-fix-turn timeout / wall-clock budget (max_attempts bounds turns, NOT wall-clock).
- "Best, not last" recovery (tag pre-amend tips `refs/a2a/attempt-N`) — handed-off = LAST attempt; the
  final suffix discloses the state; documented limitation.
- Skipping review when verify is already actionable (~halve the review cost; both run today so one fix turn
  can address verify+review together, and `last_review` stays meaningful for the suffix — stated tradeoff).
- Warm cross-turn agent (the warm-pool slice).

## Firewall
Designed from the bridge's own seams; clean-room `design`-workflow cross-check (rev2) + dual spec-review
(containerized dogfood, leak-safe post-reaper, + a2a-local codex — both confirmed amend keeps parent=
base_sha + bot identity, the synth-hoist need, and the classify direction; rev3 folds the silent-work-loss
fix, the config lifecycle, the total-helper/per-iteration-token contract, and the hand-off final-sha patch).
