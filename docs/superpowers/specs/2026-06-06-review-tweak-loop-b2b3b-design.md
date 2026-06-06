# B2b-3b — Review→Tweak Loop — Design

**Date:** 2026-06-06
**Status:** Draft (rev2 — folds the firewalled clean-room `design`-workflow cross-check + owner decisions:
AMEND commit strategy, dedicated `implement-fix` workflow). rev1 = the initial decompose.
**Builds on:** B2b-1 (ADR-0019), B2b-2 (ADR-0020), the `:ro` reaper (ADR-0021), B2b-3a (ADR-0022). Capstone
of the B2b-3 self-correcting loop.

## Goal

Make `implement` self-correcting: when the committed change FAILS verify or is REJECTED by review,
re-prompt the impl agent on the SAME persistent clone (which has the prior commit) to FIX the issues, then
re-commit / re-verify / re-review — bounded by `max_attempts`. ADVISORY: it TRIES to reach APPROVE+PASS; on
the bound it hands off the best-effort branch + the final state (the operator accepts at merge).

## Decisions (owner + clean-room cross-check)

1. **A bounded post-commit control loop** wrapping today's `Action::Commit` arm. One-time setup (clone,
   checkout base, registry/executor, `clone_cwd`, `base_sha`, configs, the graphs) stays OUTSIDE. The loop:
   `verify → review → classify → (fix turn → re-commit) → repeat`.
2. **Strict two-phase fallibility (the load-bearing invariant).** Phase 1 (pre-first-commit) keeps the
   existing `?`/fail-loud. Phase 2 (after the first `host_commit`) is **lossy**: EVERY fallible op (later
   commits, `stage_state`, `head_sha`) is `match`ed into a terminal `StopReason` and converted to a
   hand-off — never `?`, never panic. The hand-off always prints once a commit exists.
3. **AMEND into one commit** [owner; clean-room's rec]. Each fix `git commit --amend`s into a SINGLE commit
   whose parent stays `base_sha`. So the operator's `handoff_text` + `cherry-pick -n FETCH_HEAD` stay
   **byte-identical** (zero change to the invariant-critical, tested merge helper); the operator gets one
   commit (same end-state as a chain; the clone is reaped). verify runs on the committed tree; review runs
   on `base_sha..HEAD` (= base..the-amended-tip, the cumulative change). Per-attempt checkpoints are NOT
   kept in the (ephemeral) clone — the loop suffix + logs carry the history.
4. **`[implement].max_attempts` = MAX TOTAL COMMITS on the branch** (default 3 = the initial commit + up to
   2 fix amends; `max=1` ⇒ exactly today's behavior, 0 fix turns). `0` → **ConfigError** (fail-loud — a 0
   is a typo); `> HARD_MAX(10)` → clamp + an `eprintln` note. ADVISORY (exit 0; a `--gate` flag is a
   deferred slice).
5. **Dedicated `implement-fix` workflow + prompt** [owner; clean-room's rec]. A new 1-node `impl`-agent
   workflow (same shape as `implement-edit`); `prompts/implement-fix.md` is framed as "CONTINUE & fix these
   specific verify failures / review findings on the current clone; re-stage; rewrite `.git/A2A_COMMIT_MSG`;
   don't commit." Registered in `INIT_WORKFLOWS` + the containerized example. If `implement-fix` is absent
   from a loaded `wf_map` → **`FixUnavailable`** degradation (no fix turns; hand off — mirrors review's
   `NotLoaded`; never a new hard config requirement).
6. **Pure decision core `classify` (verdicts only) + structural reasons (orchestration).** `classify` sees
   only the verify+review outcomes + the attempt count; the structural failures arise in the loop body.
7. **Tweak trigger / matrix** (verified): `verify_ok = Ran(passed)|NotConfigured`; `review_ok =
   Ran{Approve}|NotConfigured` (a degraded `Approve` with `reviewers_failed>0` still counts — advisory, per
   B2b-3a). Actionable: `verify = Ran && !passed`; `review = Ran{Reject}`. `ConfigError`/`NotLoaded`/
   `Incomplete`/`Inconclusive`/timeout are **neither ok nor actionable** → `NotActionable` (fail-safe: no
   concrete failure list ⇒ re-prompting is blind thrash; the per-step suffix already carries the why).
8. **Fix context fed to attempts 2+** = the verify failure-digest + (only when review verdict == REJECT)
   the **hoisted synth body** (B2b-3a discards the full synth today — only `summary` is kept — so it must
   be hoisted to a loop-scoped local). Bounded (truncated tails), never full logs.

## Architecture

### Pure decision core — `bin/a2a-bridge/src/tweak.rs` (NEW)
```rust
pub enum StopReason {
    Success,            // verify_ok && review_ok
    BoundReached,       // actionable but attempt >= max_attempts
    NotActionable,      // Inconclusive / Incomplete / ConfigError / NotLoaded / timeout — nothing to fix
    NoProgress,         // a fix turn's decide() != Commit (agent staged nothing new)
    AmendFailed,        // host_amend_commit errored — prior sha kept
    StepError(String),  // a git step (stage_state/head_sha) errored mid-loop — prior sha kept
    FixUnavailable,     // implement-fix not registered — degrade to no-fix
}
pub enum LoopStep { Continue, Stop(StopReason) }
pub struct LoopReport { pub attempts: u32, pub stop_reason: StopReason }

/// PURE — the loop's riskiest branching, unit-tested as a verify×review cross-product matrix.
/// Order: Success → (neither actionable ⇒ NotActionable) → (would-tweak but attempt≥max ⇒ BoundReached) →
/// Continue. NoProgress/AmendFailed/StepError/FixUnavailable are NOT returned here — the orchestration
/// stamps them into LoopReport directly.
pub fn classify(attempt: u32, max_attempts: u32,
                v: &verify::VerifyOutcome, r: &review::ReviewOutcome) -> LoopStep;
/// PURE. The fix-turn input: original task + the verify failure-digest + (Some only on REJECT) the synth
/// findings; bounded.
pub fn build_fix_input(task: &str, v: &verify::VerifyOutcome,
                       review_findings: Option<&str>, max_bytes: usize) -> String;
/// PURE. The hand-off loop-outcome line: "loop: <attempts> attempt(s) — <Success|bound reached|needs human|
/// no progress|amend failed|step error|fix unavailable>".
pub fn loop_outcome_suffix(rep: &LoopReport) -> String;
```

### Pure helpers (existing modules)
- `verify.rs`: `pub fn failure_digest(verdict: &VerifyVerdict, max_bytes: usize) -> String` — the !ok GATE
  results (name + truncated output), for the fix input.
- `review.rs`: hoist the synth body (the full terminal text — already captured locally in the review block,
  just retain it loop-scoped). Reuse `verify::truncate_output`.

### `[implement]` config — `bin/a2a-bridge/src/config.rs`
```rust
pub struct ImplementToml { #[serde(default)] max_attempts: Option<u32>,
                           #[serde(default)] fix_workflow: Option<String> }
pub struct LoopConfig { pub max_attempts: u32, pub fix_workflow: bridge_core::ids::WorkflowId }
// to_config(): max_attempts absent→3, 0→ConfigError, >10→clamp(10)+eprintln; fix_workflow default
// "implement-fix", parsed to WorkflowId (validated pre-commit). RegistryConfig.implement: Option<ImplementToml>.
```

### `implement.rs` — share the lock-retry, add amend
```rust
fn host_commit_argv_run(clone, argv) -> Result<String,String>;  // the shared 5×index.lock retry + stale-clear
pub fn host_commit(clone, msg) -> ...        // = commit_argv → host_commit_argv_run (behavior-preserving refactor)
pub fn commit_amend_argv(clone, msg) -> ...  // the existing -c pins + `commit --amend`
pub fn host_amend_commit(clone, msg) -> ...  // = commit_amend_argv → host_commit_argv_run
```
`handoff_text` UNCHANGED (amend keeps the tip a direct child of base) — extend ONLY to append the loop
report (or append `tweak::loop_outcome_suffix` at the call site, leaving `handoff_text` itself untouched).

### The loop in `implement_cmd` — `main.rs`
- Extract `async fn drain_impl(stream) -> bool` (the iter-1 drain — node logging + `completed`), shared by
  the initial edit turn and every fix turn (one `?`-free zone).
- After the FIRST `host_commit` (the only post-commit `?`, from the initial `Action::Commit` arm):
```
attempt = 1; let mut last_verify; let mut last_review; let mut synth_body;
loop {
  last_verify = run_verify_step(...);                          // best-effort (B2b-2)
  (last_review, synth_body) = run_review_step(base_sha, sha);  // B2b-3a; synth hoisted
  match tweak::classify(attempt, max, &last_verify, &last_review) {
    Stop(r) => { report = LoopReport{attempts: attempt, stop_reason: r}; break }
    Continue => {
      let Some(fix_graph) = fix_graph.as_ref() else { report = …FixUnavailable; break };
      let pre_i = match head_sha(&clone) { Ok(s)=>s, Err(e)=>{ report=…StepError(e); break } };
      let _ = remove_file(.git/A2A_COMMIT_MSG);
      let findings = matches!(last_review, Ran{verdict: Reject, ..}).then(|| synth_body.as_str());
      let input = tweak::build_fix_input(&a.task, &last_verify, findings, 16*1024);
      let completed = drain_impl(executor.run_with_context(fix_graph.clone(), input,
                        format!("impl-fix-{task_id}-{attempt}"), CancellationToken::new(),
                        ctx_with(clone_cwd.clone()))).await;
      let guard = head_guard(&clone, &branch, &pre_i);
      let stage = match stage_state(&clone) { Ok(s)=>s, Err(e)=>{ report=…StepError(e); break } };
      let msg = commit_message(read_commit_msg_file(&clone), &a.task);
      match decide(completed, guard, stage, msg) {
        Action::Commit(m) => match host_amend_commit(&clone, &m) {
          Ok(s) => { let _=remove_file(A2A_COMMIT_MSG); sha=s; attempt+=1; continue }
          Err(_) => { report=…AmendFailed; break }
        }
        _ => { report=…NoProgress; break }   // no further change → stop
      }
    }
  }
}
handoff += verify::outcome_suffix(&last_verify);
handoff += review::outcome_suffix(&last_review);
handoff += tweak::loop_outcome_suffix(&report);
println!("{handoff}"); Ok(())
```
(`head_guard` per-iteration re-snapshots `pre_i` so an agent self-commit THIS turn is still caught.)

## Component / file boundaries

| Concern | Home |
|---|---|
| pure `classify`/`StopReason`/`LoopReport`/`build_fix_input`/`loop_outcome_suffix` | `bin/a2a-bridge/src/tweak.rs` (NEW) |
| pure `failure_digest`; hoist the synth body | `bin/a2a-bridge/src/verify.rs`, `review.rs` |
| `ImplementToml`/`LoopConfig` + `RegistryConfig.implement` | `bin/a2a-bridge/src/config.rs` |
| `host_commit_argv_run` refactor + `commit_amend_argv`/`host_amend_commit` | `bin/a2a-bridge/src/implement.rs` |
| `prompts/implement-fix.md` + `implement-fix` workflow (embedded + example) | `prompts/` + `main.rs` `INIT_*` + `examples/a2a-bridge.containerized.toml` |
| `drain_impl` + the loop in the `Action::Commit` arm | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) |

## Testing
- **Unit (no Docker):** `classify` **cross-product matrix** — verify {Ran-pass, Ran-fail, NotConfigured,
  ConfigError} × review {Approve, Reject, Inconclusive, NotConfigured, NotLoaded, ConfigError, Incomplete}
  × {attempt<max, attempt==max} → the right LoopStep/StopReason; `build_fix_input` (task + failure-digest +
  findings-only-on-Reject + bounding); `failure_digest` (only !ok gates, truncated); `loop_outcome_suffix`
  (each StopReason); `ImplementToml::to_config` (absent→3, 0→ConfigError, >10→clamp, fix_workflow default/
  malformed); `commit_amend_argv` (the -c pins + `--amend`) + a temp-repo test that amend folds to a SINGLE
  commit whose parent is still `base_sha`.
- **Live gate (Docker, dogfooded):** (1) right-first-try → 1 attempt, APPROVE+PASS, ONE commit, the
  unchanged merge command. (2) a verify-FAIL on attempt 1 (introduce a clippy/test break) → the fix turn
  gets the failure-digest, fixes it, attempt 2 PASS+APPROVE, still ONE commit (amended); `cherry-pick -n
  FETCH_HEAD` applies the cumulative change. (3) `max_attempts=1` + a failing task → `loop: 1 attempt —
  bound reached` + best-effort + exit 0. (4) the `:rw` fix + `:ro` review containers reaped each attempt
  (poll-to-0). (5) the commit + hand-off ALWAYS print. (Temp-repo fake-executor integration tests cover
  reject-then-approve, no-progress, amend-fails-mid-loop, agent-mutates-HEAD.)

## Deferred (only slice-sized; smaller suggestions folded inline per owner)
- A `--gate` / exit-non-zero-on-unclean flag (CI/automation).
- A per-fix-turn timeout (reuse review's `tokio::select!` cancel-then-drain) / a wall-clock budget.
- Warm cross-turn agent (keep the SAME container across attempts — the warm-pool slice; B2b-3b uses the
  persistent clone as continuity).
- Incremental latest-delta review; tree-hash no-progress detection; a machine-readable loop report.

## Firewall
Designed from the bridge's own seams; cross-checked by the bridge's firewalled clean-room `design` workflow
(independent of this spec — it converged on the spine + flipped the commit strategy to amend + added the
dedicated fix workflow, both folded). Dual spec-review = containerized dogfood (leak-safe post-reaper) +
a2a-local `codex-review`.
