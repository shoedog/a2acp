# B2b-3b — Review→Tweak Loop — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make `a2a-bridge implement` self-correcting: after the first commit, run a bounded
verify→review→classify→fix→amend loop on the SAME clone until APPROVE+PASS or `[implement].max_attempts`,
then hand off the best-effort branch + the final state. Advisory (exit 0).

**Architecture:** A pure decision core (`tweak.rs` — `classify`/`fix_step`/`build_fix_input`/
`loop_outcome_suffix`) drives a thin async loop in `implement_cmd`. The first commit stays fail-loud (phase
1); everything after it is lossy (phase 2 — no `?`/panic, every fallible op → a `StopReason`). Each fix
AMENDs into the single commit (parent stays `base_sha`, original message kept) so the operator hand-off is
byte-identical. A fix-turn that mutates HEAD trips the guard → `git reset --hard <last_good_sha>` (no work
loss). Builds on B2b-1/-2/-3a + the `:ro` reaper.

**Tech Stack:** Rust (workspace), `bin/a2a-bridge` (config.rs, implement.rs, verify.rs, review.rs, the new
tweak.rs, main.rs), `bridge_workflow::executor`, the ContainerRw `impl` agent, Docker (live gate only).

**Spec:** `docs/superpowers/specs/2026-06-06-review-tweak-loop-b2b3b-design.md` (rev3, dual-reviewed).

**Conventions:** TDD green-per-task; task/code commits do NOT carry the `Co-Authored-By` trailer (the doc
commits do). Coverage after `cargo llvm-cov clean --workspace`; floors per **ci.yml** (workspace 85,
bridge-core/acp/api/workflow 90 — the bin crate has no per-crate floor; keep `tweak.rs`/`verify.rs`/
`config.rs` pure helpers ≥95). Run on branch `feat/implement-b2b3b` off `main`.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `bin/a2a-bridge/src/tweak.rs` | **CREATE** | Pure loop core: `StopReason`/`LoopStep`/`LoopReport`/`FixDisposition`, `classify`, `fix_step`, `build_fix_input`, `loop_outcome_suffix`. |
| `bin/a2a-bridge/src/verify.rs` | modify | Add `VerifyOutcome::Incomplete` + its `outcome_suffix` arm; add pure `failure_digest`. |
| `bin/a2a-bridge/src/config.rs` | modify | `ImplementToml`/`LoopConfig`(+`Default`)/`to_config` + `RegistryConfig.implement`. |
| `bin/a2a-bridge/src/implement.rs` | modify | Extract `host_commit_argv_run`; add `commit_amend_argv`/`host_amend_commit`/`reset_hard`/`reset_worktree_to_head`. |
| `bin/a2a-bridge/src/main.rs` | modify | `mod tweak;`; `drain_impl`; total `run_verify_step`/`run_review_step`; the loop in `implement_cmd`. |
| `prompts/implement-fix.md` | **CREATE** | The fix-turn prompt (continue & fix; re-stage; don't commit; no message). |
| `examples/a2a-bridge.containerized.toml` | modify | `[implement]` block + the `implement-fix` workflow (example-only). |

---

## Task 1: `VerifyOutcome::Incomplete` + `failure_digest` (pure, verify.rs)

**Files:**
- Modify: `bin/a2a-bridge/src/verify.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `verify.rs` tests:

```rust
    #[test]
    fn outcome_suffix_incomplete() {
        assert_eq!(
            outcome_suffix(&VerifyOutcome::Incomplete),
            "verify: incomplete (did not finish)"
        );
    }

    #[test]
    fn failure_digest_only_failed_gates_with_budget() {
        // a passed gate is omitted; a failed gate is included with its (truncated) output.
        let v = aggregate(vec![
            VerifyResult { name: "fmt".into(), gate: true, ok: true, output: "ok".into() },
            VerifyResult { name: "clippy".into(), gate: true, ok: false, output: "E".repeat(50) },
        ]);
        let d = failure_digest(&v, 20);
        assert!(d.contains("### clippy"));
        assert!(!d.contains("### fmt"));
        assert!(d.contains("truncated")); // output bounded
    }

    #[test]
    fn failure_digest_empty_when_no_gate_failures() {
        let v = aggregate(vec![
            VerifyResult { name: "test".into(), gate: true, ok: true, output: "ok".into() },
            VerifyResult { name: "cov".into(), gate: false, ok: false, output: "x".into() },
        ]);
        assert_eq!(failure_digest(&v, 4096), ""); // non-gate failure is NOT in the digest
    }

    #[test]
    fn failure_digest_empty_output_placeholder() {
        let v = aggregate(vec![
            VerifyResult { name: "build".into(), gate: true, ok: false, output: "   ".into() },
        ]);
        assert!(failure_digest(&v, 4096).contains("(no output)"));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p a2a-bridge --bin a2a-bridge verify::tests::failure_digest 2>&1 | tail -20`
Expected: FAIL — `VerifyOutcome::Incomplete` and `failure_digest` don't exist.

- [ ] **Step 3: Implement**

In `verify.rs`, add the `Incomplete` variant to the enum (after `ConfigError`):

```rust
pub enum VerifyOutcome {
    Ran(VerifyVerdict),
    NotConfigured,
    /// The `[verify]` block failed validation; the detail is logged to stderr at the call site.
    ConfigError,
    /// The step did not run to completion (e.g. a pre-verify worktree reset failed) — the loop sentinel
    /// and the catch-all so the always-print hand-off has a defined value. (B2b-3b.)
    Incomplete,
}
```

Add the `Incomplete` arm to `outcome_suffix`:

```rust
        VerifyOutcome::Incomplete => "verify: incomplete (did not finish)".to_string(),
```

Add the pure `failure_digest` (after `truncate_output`):

```rust
/// PURE. The fix-turn digest: ONLY the GATE failures (the ones that fail the verdict + drive `actionable`),
/// in order, each `### <name>` + its (truncated) output. Non-gate failures are reported in the hand-off
/// but never re-prompted. Empty when no gate failed. `run_verify` stops at the first gate failure, so this
/// is normally one entry; the per-result budget splits `max_bytes` evenly across however many there are.
pub fn failure_digest(v: &VerifyVerdict, max_bytes: usize) -> String {
    let failed: Vec<&VerifyResult> = v.results.iter().filter(|r| r.gate && !r.ok).collect();
    if failed.is_empty() {
        return String::new();
    }
    let per = (max_bytes / failed.len()).max(1);
    let mut s = String::new();
    for r in failed {
        s.push_str("### ");
        s.push_str(&r.name);
        s.push('\n');
        let body = if r.output.trim().is_empty() { "(no output)" } else { &r.output };
        s.push_str(&truncate_output(body, per));
        s.push('\n');
    }
    s
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p a2a-bridge --bin a2a-bridge verify:: 2>&1 | tail -20`
Expected: PASS (all verify tests, including the existing `outcome_suffix_covers_three_arms`).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/verify.rs
git commit -m "verify: add VerifyOutcome::Incomplete + pure failure_digest (b2b3b)"
```

---

## Task 2: `[implement]` config — `ImplementToml`/`LoopConfig` (config.rs)

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs`
- Test: same file

- [ ] **Step 1: Write the failing tests**

Add to the `config.rs` `tests` module:

```rust
    #[test]
    fn implement_config_defaults_when_absent() {
        let lc = ImplementToml { max_attempts: None, fix_workflow: None }
            .to_config()
            .unwrap();
        assert_eq!(lc.max_attempts, 3);
        assert_eq!(lc.fix_workflow.as_str(), "implement-fix");
        // LoopConfig::default() matches the absent-block behavior.
        assert_eq!(LoopConfig::default().max_attempts, 3);
        assert_eq!(LoopConfig::default().fix_workflow.as_str(), "implement-fix");
    }

    #[test]
    fn implement_config_max_attempts_zero_is_error() {
        assert!(ImplementToml { max_attempts: Some(0), fix_workflow: None }
            .to_config()
            .is_err());
    }

    #[test]
    fn implement_config_clamps_above_hard_max() {
        let lc = ImplementToml { max_attempts: Some(99), fix_workflow: None }
            .to_config()
            .unwrap();
        assert_eq!(lc.max_attempts, 10);
    }

    #[test]
    fn implement_config_custom_fix_workflow_and_malformed() {
        let lc = ImplementToml { max_attempts: Some(2), fix_workflow: Some("my-fix".into()) }
            .to_config()
            .unwrap();
        assert_eq!(lc.max_attempts, 2);
        assert_eq!(lc.fix_workflow.as_str(), "my-fix");
        // empty id is malformed.
        assert!(ImplementToml { max_attempts: None, fix_workflow: Some("".into()) }
            .to_config()
            .is_err());
    }

    #[test]
    fn implement_block_parses_from_toml() {
        let c = RegistryConfig::parse(
            "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n\
             [implement]\nmax_attempts=2\nfix_workflow=\"implement-fix\"\n",
        )
        .unwrap();
        let lc = c.implement.as_ref().unwrap().to_config().unwrap();
        assert_eq!(lc.max_attempts, 2);
        // absent [implement] → None (the call site maps to LoopConfig::default()).
        let c2 = RegistryConfig::parse(
            "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n",
        )
        .unwrap();
        assert!(c2.implement.is_none());
    }
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p a2a-bridge --bin a2a-bridge config::tests::implement 2>&1 | tail -20`
Expected: FAIL — `ImplementToml`/`LoopConfig` don't exist.

- [ ] **Step 3: Implement**

In `config.rs`, add the `implement` field to `RegistryConfig` (after `review`):

```rust
    /// `[implement]` (Slice B2b-3b): the review→tweak loop config. Absent → `LoopConfig::default()`.
    #[serde(default)]
    pub implement: Option<ImplementToml>,
```

Add the types + conversion (place after the `ReviewConfig`/`ReviewToml` block, ~line 336):

```rust
/// `[implement]` (Slice B2b-3b): bounds + names the fix workflow for the review→tweak loop.
#[derive(Debug, serde::Deserialize)]
pub struct ImplementToml {
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub fix_workflow: Option<String>,
}

/// Parsed `[implement]`: a validated max + a parsed fix-workflow id (so the post-commit lookup is a soft
/// `FixUnavailable`, never an abort). A malformed block is fail-loud PRE-commit (resolved before the clone).
#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub max_attempts: u32,
    pub fix_workflow: bridge_core::ids::WorkflowId,
}

/// The default fix-workflow id (also the absent-block fallback). Centralizes the literal.
fn default_fix_workflow_id() -> bridge_core::ids::WorkflowId {
    bridge_core::ids::WorkflowId::parse("implement-fix").expect("static id is valid")
}

/// Belt-and-suspenders cap on commit turns (an over-large `max_attempts` is clamped, not rejected).
const IMPLEMENT_HARD_MAX: u32 = 10;

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            fix_workflow: default_fix_workflow_id(),
        }
    }
}

impl ImplementToml {
    pub fn to_config(&self) -> Result<LoopConfig, ConfigError> {
        let max_attempts = match self.max_attempts {
            None => 3,
            Some(0) => {
                return Err(ConfigError::Registry(
                    "[implement] max_attempts must be >= 1".into(),
                ))
            }
            Some(n) if n > IMPLEMENT_HARD_MAX => {
                eprintln!("[implement] max_attempts {n} > {IMPLEMENT_HARD_MAX}; clamping to {IMPLEMENT_HARD_MAX}");
                IMPLEMENT_HARD_MAX
            }
            Some(n) => n,
        };
        let fix_workflow = match &self.fix_workflow {
            Some(s) => bridge_core::ids::WorkflowId::parse(s.clone())
                .map_err(|e| ConfigError::Registry(format!("[implement] fix_workflow id: {e:?}")))?,
            None => default_fix_workflow_id(),
        };
        Ok(LoopConfig {
            max_attempts,
            fix_workflow,
        })
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p a2a-bridge --bin a2a-bridge config::tests::implement 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "config: [implement] block + LoopConfig (b2b3b)"
```

---

## Task 3: pure loop core — `tweak.rs` (CREATE)

**Files:**
- Create: `bin/a2a-bridge/src/tweak.rs`
- Modify: `bin/a2a-bridge/src/main.rs` (add `mod tweak;`)

- [ ] **Step 1: Create the module with its tests (failing)**

Create `bin/a2a-bridge/src/tweak.rs`:

```rust
//! The B2b-3b review→tweak loop's PURE decision core. The async loop in `implement_cmd` is thin glue over
//! these: `classify` (verify+review → keep going / stop), `fix_step` (the post-fix-turn action → amend /
//! diverged / no-progress — the no-work-loss-critical mapping), `build_fix_input`, `loop_outcome_suffix`.
//! All pure + unit-tested (no panics, no slicing — per B2b-3a's em-dash lesson).

use crate::review::{ReviewOutcome, Verdict};
use crate::verify::VerifyOutcome;

/// Why the loop stopped. Drives the hand-off suffix + the operator's read of the final state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Success,        // verify PASS (or n/a) AND review APPROVE (or n/a)
    BoundReached,   // still actionable at attempt == max_attempts
    NotActionable,  // no concrete failure to re-prompt on (Inconclusive/Incomplete/ConfigError/…)
    NoProgress,     // a fix turn staged nothing new (NoCommitClean/Dirty)
    HeadMutated,    // a fix turn advanced/switched HEAD (self-commit) — reset to last-good, distinct stop
    AmendFailed,    // host_amend_commit errored
    StepError(String), // a post-commit git op (reset/stage/head) failed — reduced, never `?`
    FixUnavailable, // actionable but no fix workflow is registered
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopStep {
    Continue,
    Stop(StopReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopReport {
    pub attempts: u32,
    pub stop_reason: StopReason,
}

/// What a fix turn's `decide()` Action means for the loop. Isolated + tested because mapping `Abort` to
/// the wrong branch silently drops the cumulative tree (the B2b-3b blocker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixDisposition {
    Amend,      // Action::Commit — fold the fix into the single commit
    Diverged,   // Action::Abort — agent mutated HEAD; reset to last-good + HeadMutated
    NoProgress, // Action::NoCommitClean/Dirty — agent staged nothing
}

/// PURE. `Commit → Amend`, `Abort → Diverged`, `NoCommit* → NoProgress`.
pub fn fix_step(action: &crate::implement::Action) -> FixDisposition {
    use crate::implement::Action;
    match action {
        Action::Commit(_) => FixDisposition::Amend,
        Action::Abort(_) => FixDisposition::Diverged,
        Action::NoCommitClean | Action::NoCommitDirty => FixDisposition::NoProgress,
    }
}

/// PURE. Decide the next step from this attempt's verify + review outcomes.
/// - ok: verify ∈ {Ran(passed), NotConfigured}; review ∈ {Ran(Approve incl. reviewers_failed>0), NotConfigured}.
/// - actionable: verify Ran && !passed, OR review Ran(Reject). Anything else (Inconclusive / Incomplete /
///   ConfigError / NotLoaded) is NEITHER ok nor actionable → NotActionable (re-prompting blind would thrash).
pub fn classify(
    attempt: u32,
    max_attempts: u32,
    v: &VerifyOutcome,
    r: &ReviewOutcome,
) -> LoopStep {
    let verify_ok = match v {
        VerifyOutcome::Ran(verdict) => verdict.passed,
        VerifyOutcome::NotConfigured => true,
        VerifyOutcome::ConfigError | VerifyOutcome::Incomplete => false,
    };
    let review_ok = match r {
        ReviewOutcome::Ran { verdict, .. } => matches!(verdict, Verdict::Approve),
        ReviewOutcome::NotConfigured => true,
        ReviewOutcome::ConfigError | ReviewOutcome::NotLoaded | ReviewOutcome::Incomplete => false,
    };
    if verify_ok && review_ok {
        return LoopStep::Stop(StopReason::Success);
    }
    let verify_actionable = matches!(v, VerifyOutcome::Ran(verdict) if !verdict.passed);
    let review_actionable = matches!(
        r,
        ReviewOutcome::Ran { verdict: Verdict::Reject, .. }
    );
    if !(verify_actionable || review_actionable) {
        return LoopStep::Stop(StopReason::NotActionable);
    }
    if attempt >= max_attempts {
        return LoopStep::Stop(StopReason::BoundReached);
    }
    LoopStep::Continue
}

/// PURE. The fix turn's `{{input}}`: the task in full + the verify failure digest + (only on REJECT) the
/// hoisted synth body. Task kept whole; the remaining budget splits across the present blocks (verify first).
pub fn build_fix_input(
    task: &str,
    verify_digest: &str,
    review_findings: Option<&str>,
    max_bytes: usize,
) -> String {
    let header = format!(
        "{task}\n\nThe previous attempt did not pass. FIX the issues below on the current clone (it already \
         has your prior commit); re-stage your fixes with `git add`; do NOT run `git commit` and do NOT \
         write a commit message.\n"
    );
    let remaining = max_bytes.saturating_sub(header.len());
    let v = verify_digest.trim();
    let rfind = review_findings.map(str::trim).filter(|s| !s.is_empty());
    let (vbud, rbud) = match (!v.is_empty(), rfind.is_some()) {
        (true, true) => (remaining / 2, remaining - remaining / 2),
        (true, false) => (remaining, 0),
        (false, true) => (0, remaining),
        (false, false) => (0, 0),
    };
    let mut out = header;
    if !v.is_empty() {
        out.push_str("\n## Verify failures\n");
        out.push_str(&crate::verify::truncate_output(v, vbud));
        out.push('\n');
    }
    if let Some(rf) = rfind {
        out.push_str("\n## Review findings (REJECTED)\n");
        out.push_str(&crate::verify::truncate_output(rf, rbud));
        out.push('\n');
    }
    out
}

/// PURE. The one-line loop suffix appended to the hand-off (after the verify + review suffixes).
pub fn loop_outcome_suffix(rep: &LoopReport) -> String {
    let why = match &rep.stop_reason {
        StopReason::Success => "converged".to_string(),
        StopReason::BoundReached => "bound reached".to_string(),
        StopReason::NotActionable => "no actionable signal".to_string(),
        StopReason::NoProgress => "fix turn staged nothing".to_string(),
        StopReason::HeadMutated => "fix turn diverged HEAD — reset to last-good".to_string(),
        StopReason::AmendFailed => "amend failed".to_string(),
        StopReason::FixUnavailable => "no fix workflow configured".to_string(),
        StopReason::StepError(e) => format!("step error: {e}"),
    };
    format!("loop: {} attempt(s) — {}", rep.attempts, why)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::{aggregate, VerifyResult};

    fn ran(passed: bool) -> VerifyOutcome {
        VerifyOutcome::Ran(aggregate(vec![VerifyResult {
            name: "test".into(),
            gate: true,
            ok: passed,
            output: String::new(),
        }]))
    }
    fn rev(v: Verdict, failed: usize) -> ReviewOutcome {
        ReviewOutcome::Ran {
            verdict: v,
            summary: "s".into(),
            reviewers_failed: failed,
        }
    }

    #[test]
    fn success_when_both_ok_incl_degraded_approve() {
        assert_eq!(
            classify(1, 3, &ran(true), &rev(Verdict::Approve, 0)),
            LoopStep::Stop(StopReason::Success)
        );
        // a degraded APPROVE (a reviewer leg failed) is still ok.
        assert_eq!(
            classify(1, 3, &ran(true), &rev(Verdict::Approve, 1)),
            LoopStep::Stop(StopReason::Success)
        );
        // both not configured → success on attempt 1 (== today's no-verify/no-review behavior).
        assert_eq!(
            classify(1, 1, &VerifyOutcome::NotConfigured, &ReviewOutcome::NotConfigured),
            LoopStep::Stop(StopReason::Success)
        );
    }

    #[test]
    fn continue_when_actionable_under_bound() {
        // verify fail, review approve → fix the build.
        assert_eq!(
            classify(1, 3, &ran(false), &rev(Verdict::Approve, 0)),
            LoopStep::Continue
        );
        // verify pass, review reject → fix the findings.
        assert_eq!(
            classify(1, 3, &ran(true), &rev(Verdict::Reject, 0)),
            LoopStep::Continue
        );
        // reject with a failed reviewer leg is still actionable.
        assert_eq!(
            classify(1, 3, &ran(true), &rev(Verdict::Reject, 1)),
            LoopStep::Continue
        );
        // verify fail, review not configured → actionable.
        assert_eq!(
            classify(1, 3, &ran(false), &ReviewOutcome::NotConfigured),
            LoopStep::Continue
        );
    }

    #[test]
    fn bound_reached_at_max() {
        assert_eq!(
            classify(3, 3, &ran(false), &rev(Verdict::Reject, 0)),
            LoopStep::Stop(StopReason::BoundReached)
        );
        // max=1 + a failing task → bound on attempt 1.
        assert_eq!(
            classify(1, 1, &ran(false), &ReviewOutcome::NotConfigured),
            LoopStep::Stop(StopReason::BoundReached)
        );
    }

    #[test]
    fn not_actionable_on_inconclusive_or_incomplete_or_configerror() {
        // Inconclusive review + passing verify → not ok (not Approve), not actionable (not Reject).
        assert_eq!(
            classify(1, 3, &ran(true), &rev(Verdict::Inconclusive, 0)),
            LoopStep::Stop(StopReason::NotActionable)
        );
        // verify Incomplete + review Approve → verify not ok, but not actionable → stop.
        assert_eq!(
            classify(1, 3, &VerifyOutcome::Incomplete, &rev(Verdict::Approve, 0)),
            LoopStep::Stop(StopReason::NotActionable)
        );
        // verify ConfigError + review Incomplete → neither ok nor actionable.
        assert_eq!(
            classify(1, 3, &VerifyOutcome::ConfigError, &ReviewOutcome::Incomplete),
            LoopStep::Stop(StopReason::NotActionable)
        );
        // review NotLoaded + verify NotConfigured → verify ok but review not ok, not actionable → stop.
        assert_eq!(
            classify(1, 3, &VerifyOutcome::NotConfigured, &ReviewOutcome::NotLoaded),
            LoopStep::Stop(StopReason::NotActionable)
        );
    }

    #[test]
    fn fix_step_maps_each_action() {
        use crate::implement::Action;
        assert_eq!(fix_step(&Action::Commit("m".into())), FixDisposition::Amend);
        assert_eq!(fix_step(&Action::Abort("x".into())), FixDisposition::Diverged);
        assert_eq!(fix_step(&Action::NoCommitClean), FixDisposition::NoProgress);
        assert_eq!(fix_step(&Action::NoCommitDirty), FixDisposition::NoProgress);
    }

    #[test]
    fn build_fix_input_keeps_task_and_sections() {
        // both blocks present
        let i = build_fix_input("do X", "### clippy\nerr", Some("BLOCKER: bug"), 4096);
        assert!(i.contains("do X"));
        assert!(i.contains("## Verify failures") && i.contains("### clippy"));
        assert!(i.contains("## Review findings (REJECTED)") && i.contains("BLOCKER: bug"));
        // verify-only (no review findings)
        let v = build_fix_input("do X", "### test\nfail", None, 4096);
        assert!(v.contains("## Verify failures") && !v.contains("Review findings"));
        // review-only (empty digest)
        let r = build_fix_input("do X", "", Some("MAJOR: y"), 4096);
        assert!(!r.contains("## Verify failures") && r.contains("Review findings"));
        // a tiny budget never panics + keeps the task
        let t = build_fix_input("do X", &"E".repeat(9000), Some(&"R".repeat(9000)), 256);
        assert!(t.contains("do X"));
    }

    #[test]
    fn loop_outcome_suffix_all_reasons() {
        let mk = |r: StopReason| loop_outcome_suffix(&LoopReport { attempts: 2, stop_reason: r });
        assert_eq!(mk(StopReason::Success), "loop: 2 attempt(s) — converged");
        assert!(mk(StopReason::BoundReached).contains("bound reached"));
        assert!(mk(StopReason::NotActionable).contains("no actionable"));
        assert!(mk(StopReason::NoProgress).contains("staged nothing"));
        assert!(mk(StopReason::HeadMutated).contains("diverged HEAD"));
        assert!(mk(StopReason::AmendFailed).contains("amend failed"));
        assert!(mk(StopReason::FixUnavailable).contains("no fix workflow"));
        assert!(mk(StopReason::StepError("boom".into())).contains("boom"));
    }
}
```

- [ ] **Step 2: Wire the module**

In `main.rs`, add `mod tweak;` beside the other `mod` decls (after `mod review;`, ~line 30).

- [ ] **Step 3: Run to verify the tests pass**

Run: `cargo test -p a2a-bridge --bin a2a-bridge tweak:: 2>&1 | tail -25`
Expected: PASS (all tweak tests). If `crate::implement::Action` isn't `PartialEq`/`Clone` enough for the
asserts — it already derives `Debug, PartialEq, Eq` (see implement.rs); no change needed.

- [ ] **Step 4: Commit**

```bash
git add bin/a2a-bridge/src/tweak.rs bin/a2a-bridge/src/main.rs
git commit -m "tweak: pure loop core (classify/fix_step/build_fix_input/suffix) (b2b3b)"
```

---

## Task 4: amend + reset git ops (implement.rs, temp-repo tested)

**Files:**
- Modify: `bin/a2a-bridge/src/implement.rs`
- Test: same file

- [ ] **Step 1: Write the failing tests**

Add to the `implement.rs` tests (the impure/temp-repo section):

```rust
    #[test]
    fn commit_amend_argv_pins_and_amends_no_edit() {
        let a = commit_amend_argv("/c");
        let joined = a.join(" ");
        assert!(joined.contains("-c safe.directory=/c"));
        assert!(joined.contains("-c core.hooksPath=/dev/null"));
        assert!(joined.contains("-c user.name=a2a-implement"));
        let ci = a.iter().position(|x| x == "commit").unwrap();
        assert_eq!(&a[ci..], &["commit", "--no-verify", "--amend", "--no-edit"]);
    }

    #[test]
    fn host_amend_folds_into_one_commit_keeping_parent_and_message() {
        let (_g, p) = temp_repo();
        let base = head_sha(&p).unwrap();
        run_git(Some(&p), &["checkout", "-q", "-b", "implement/x"]).unwrap();
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        run_git(Some(&p), &["add", "A.md"]).unwrap();
        let sha1 = host_commit(&p, "feat: the change").unwrap();
        // a SECOND staged file, then AMEND.
        std::fs::write(p.join("B.md"), "b\n").unwrap();
        run_git(Some(&p), &["add", "B.md"]).unwrap();
        let sha2 = host_amend_commit(&p).unwrap();
        assert_ne!(sha1, sha2, "amend rewrites the tip");
        // still exactly ONE commit ahead of base.
        let count = run_git(Some(&p), &["rev-list", "--count", &format!("{base}..HEAD")]).unwrap();
        assert_eq!(String::from_utf8_lossy(&count.stdout).trim(), "1");
        // parent is still base.
        let parent = run_git(Some(&p), &["rev-parse", "HEAD^"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&parent.stdout).trim(), base);
        // message kept (no per-fix message).
        let subj = run_git(Some(&p), &["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&subj.stdout).trim(), "feat: the change");
        // tree has BOTH files; author stays the bot.
        assert!(p.join("A.md").exists() && p.join("B.md").exists());
        let an = run_git(Some(&p), &["log", "-1", "--format=%an"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&an.stdout).trim(), "a2a-implement");
    }

    #[test]
    fn reset_worktree_to_head_discards_unstaged_and_untracked() {
        let (_g, p) = temp_repo();
        run_git(Some(&p), &["checkout", "-q", "-b", "implement/x"]).unwrap();
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        run_git(Some(&p), &["add", "A.md"]).unwrap();
        host_commit(&p, "feat").unwrap();
        // dirty: modify a tracked file (unstaged) + add an untracked file.
        std::fs::write(p.join("A.md"), "MUTATED\n").unwrap();
        std::fs::write(p.join("scratch.tmp"), "junk\n").unwrap();
        assert_ne!(stage_state(&p).unwrap(), StageState::Clean);
        reset_worktree_to_head(&p).unwrap();
        assert_eq!(stage_state(&p).unwrap(), StageState::Clean);
        assert_eq!(std::fs::read_to_string(p.join("A.md")).unwrap(), "a\n"); // reverted
        assert!(!p.join("scratch.tmp").exists()); // cleaned
    }

    #[test]
    fn reset_hard_restores_a_trusted_tip_after_a_rogue_commit() {
        let (_g, p) = temp_repo();
        run_git(Some(&p), &["checkout", "-q", "-b", "implement/x"]).unwrap();
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        run_git(Some(&p), &["add", "A.md"]).unwrap();
        let good = host_commit(&p, "feat").unwrap();
        // simulate an agent self-commit (the loop's HeadMutated case).
        std::fs::write(p.join("rogue.md"), "r\n").unwrap();
        run_git(Some(&p), &["add", "rogue.md"]).unwrap();
        run_git(Some(&p), &["commit", "-q", "-m", "rogue"]).unwrap();
        assert_ne!(head_sha(&p).unwrap(), good);
        reset_hard(&p, &good).unwrap();
        assert_eq!(head_sha(&p).unwrap(), good); // back to the trusted cumulative tip
        assert!(p.join("A.md").exists() && !p.join("rogue.md").exists());
    }
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p a2a-bridge --bin a2a-bridge implement::tests::host_amend 2>&1 | tail -20`
Expected: FAIL — the new fns don't exist.

- [ ] **Step 3: Implement**

In `implement.rs`, refactor `host_commit` to share a runner + add the new fns. Replace the existing
`host_commit` (lines ~224-248) with:

```rust
/// Run a prepared commit argv with `git -C <clone>`: the bounded index-lock retry, the stale-`.git/index.
/// lock` clear after retries, and reading the new sha. Shared by `host_commit` (fresh) + `host_amend_commit`.
fn host_commit_argv_run(clone: &Path, argv: &[String]) -> Result<String, String> {
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    for _ in 0..5 {
        let out = run_git(Some(clone), &refs).map_err(|e| format!("git commit: {e}"))?;
        if out.status.success() {
            return head_sha(clone);
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if !(err.contains("index.lock") || err.contains("Another git process")) {
            return Err(format!("git commit failed: {}", err.trim()));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = std::fs::remove_file(clone.join(".git").join("index.lock")); // stale-lock clear, last resort
    let out = run_git(Some(clone), &refs).map_err(|e| format!("git commit: {e}"))?;
    if out.status.success() {
        head_sha(clone)
    } else {
        Err(format!(
            "git commit failed after lock retries: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Deterministically commit the AGENT-STAGED index with the bot identity + the full hook/sign/ownership
/// pins. Returns the new commit sha. Stages nothing.
pub fn host_commit(clone: &Path, msg: &str) -> Result<String, String> {
    host_commit_argv_run(clone, &commit_argv(&clone.to_string_lossy(), msg))
}

/// `commit --amend --no-edit` argv with the SAME pins as `commit_argv` (run with `git -C <clone>`). Folds
/// the freshly-staged fix into the single commit, KEEPING the stored message (no per-fix commit message) and
/// the parent — so the operator hand-off (`cherry-pick -n FETCH_HEAD`) stays byte-identical across attempts.
pub fn commit_amend_argv(clone: &str) -> Vec<String> {
    vec![
        "-c".into(),
        format!("safe.directory={clone}"),
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "-c".into(),
        "commit.gpgsign=false".into(),
        "-c".into(),
        format!("user.name={BOT_NAME}"),
        "-c".into(),
        format!("user.email={BOT_EMAIL}"),
        "commit".into(),
        "--no-verify".into(),
        "--amend".into(),
        "--no-edit".into(),
    ]
}

/// Amend the agent-staged fix into the single commit (keeps the original message + parent + bot identity).
pub fn host_amend_commit(clone: &Path) -> Result<String, String> {
    host_commit_argv_run(clone, &commit_amend_argv(&clone.to_string_lossy()))
}

/// `git reset --hard <target>` (with the safe.directory pin). Internal helper for the two public resets.
fn reset_hard_to(clone: &Path, target: &str) -> Result<(), String> {
    let sd = format!("safe.directory={}", clone.to_string_lossy());
    git_ok(Some(clone), &["-c", &sd, "reset", "--hard", target]).map(|_| ())
}

/// Restore a trusted commit (the loop's last-good tip) after a fix turn mutated HEAD — preserves the
/// cumulative committed tree instead of letting the rogue tip reach the hand-off (the B2b-3b blocker fix).
pub fn reset_hard(clone: &Path, sha: &str) -> Result<(), String> {
    reset_hard_to(clone, sha)
}

/// Reset the working tree to the committed HEAD: discard unstaged tracked changes (`reset --hard HEAD`) and
/// untracked files (`clean -fdq`) so VERIFY tests EXACTLY the committed tree, not the agent's leftover scratch.
pub fn reset_worktree_to_head(clone: &Path) -> Result<(), String> {
    reset_hard_to(clone, "HEAD")?;
    let sd = format!("safe.directory={}", clone.to_string_lossy());
    git_ok(Some(clone), &["-c", &sd, "clean", "-fdq"]).map(|_| ())
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p a2a-bridge --bin a2a-bridge implement:: 2>&1 | tail -25`
Expected: PASS (new + all existing implement tests, incl. `host_commit_pins_neutralize_all_hooks…`).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/implement.rs
git commit -m "implement: amend + reset-to-head/reset-hard git ops (b2b3b)"
```

---

## Task 5: extract total step helpers in main.rs (`drain_impl`, `run_verify_step`, `run_review_step`)

This refactor preserves today's behavior (verify + review still run once after the first commit) while
moving the inline blocks into total helpers the loop will call each attempt. No new behavior yet.

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1: Add `drain_impl`**

Beside `drain_review` (~line 436) add:

```rust
/// Drain the implement-edit / implement-fix workflow stream → `completed`. Shared by the first edit turn and
/// every fix turn; keeps polling to the end so the executor runs its cancel cleanup. Total (no `?`).
async fn drain_impl(mut stream: bridge_workflow::executor::WorkflowStream) -> bool {
    use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
    use futures::StreamExt;
    let mut completed = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(WorkflowEvent::NodeStarted { node }) => {
                eprintln!("[implement] node {} started", node.as_str())
            }
            Ok(WorkflowEvent::NodeFinished { node, ok, .. }) => eprintln!(
                "[implement] node {} {}",
                node.as_str(),
                if ok { "ok" } else { "failed" }
            ),
            Ok(WorkflowEvent::Terminal { outcome, .. }) => {
                completed = matches!(outcome, WorkflowOutcome::Completed)
            }
            Err(e) => eprintln!("[implement] error: {e:?}"),
        }
    }
    completed
}
```

- [ ] **Step 2: Add `run_verify_step` (total)**

Below `drain_impl`:

```rust
/// Run the B2b-2 verify once (total — never `?`; reduces a config error to `ConfigError`). The verdict run
/// itself never fails (a runner error becomes a failed result). `verify_cfg` was captured pre-snapshot.
fn run_verify_step(
    verify_cfg: &Option<Result<config::VerifyConfig, config::ConfigError>>,
    clone_cwd: &bridge_core::SessionCwd,
    repo: &std::path::Path,
) -> verify::VerifyOutcome {
    match verify_cfg {
        None => verify::VerifyOutcome::NotConfigured,
        Some(Err(e)) => {
            eprintln!("[implement] verify: config error: {e:?} — skipping verify");
            verify::VerifyOutcome::ConfigError
        }
        Some(Ok(vcfg)) => {
            let repo_canon = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
            let cache_vol = verify::cache_volume_name(&vcfg.cache, &repo_canon.to_string_lossy());
            eprintln!(
                "[implement] verify: running {} command(s) in {}",
                vcfg.commands.len(),
                vcfg.image
            );
            let verdict =
                verify::run_verify(vcfg, clone_cwd, &cache_vol, &verify::docker_runner, 16 * 1024);
            for r in &verdict.results {
                if !r.ok {
                    eprintln!("[implement] verify: {} failed:\n{}", r.name, r.output);
                }
            }
            verify::VerifyOutcome::Ran(verdict)
        }
    }
}
```

- [ ] **Step 3: Add `run_review_step` (total, attempt-qualified, fresh token + cancel-drain)**

```rust
/// Run the B2b-3a review once (total). Returns `(outcome, synth_body)` — the synth is hoisted so the loop can
/// feed a REJECT's findings to the fix turn. Fresh `CancellationToken` + `select!` timeout→cancel→keep-drain
/// PER call (so the `:ro` reaper still fires on a timed-out attempt). `run_id` is qualified by `attempt`.
#[allow(clippy::too_many_arguments)]
async fn run_review_step(
    review_cfg: &Option<Result<config::ReviewConfig, config::ConfigError>>,
    wf_map: &std::collections::HashMap<
        bridge_core::ids::WorkflowId,
        std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
    >,
    executor: &bridge_workflow::executor::WorkflowExecutor,
    task: &str,
    base_sha: &str,
    head_sha: &str,
    clone_cwd: &bridge_core::SessionCwd,
    task_id: &str,
    attempt: u32,
) -> (review::ReviewOutcome, String) {
    let rcfg = match review_cfg {
        None => return (review::ReviewOutcome::NotConfigured, String::new()),
        Some(Err(e)) => {
            eprintln!("[implement] review: config error: {e:?}");
            return (review::ReviewOutcome::ConfigError, String::new());
        }
        Some(Ok(c)) => c,
    };
    let Some(graph) = wf_map.get(&rcfg.workflow).cloned() else {
        return (review::ReviewOutcome::NotLoaded, String::new());
    };
    let input = review::build_review_input(task, base_sha, head_sha);
    let ctx = bridge_workflow::executor::WorkflowRunContext {
        session_cwd: Some(clone_cwd.clone()),
    };
    let token = tokio_util::sync::CancellationToken::new();
    let stream = executor.run_with_context(
        graph,
        input,
        format!("impl-review-{task_id}-{attempt}"),
        token.clone(),
        ctx,
    );
    eprintln!("[implement] review: running implement-review (attempt {attempt})");
    let mut drain = std::pin::pin!(drain_review(stream));
    let (completed, synth, reviewers_failed) = tokio::select! {
        r = &mut drain => r,
        _ = tokio::time::sleep(rcfg.timeout) => {
            eprintln!("[implement] review: timed out after {:?}", rcfg.timeout);
            token.cancel();
            (&mut drain).await
        }
    };
    if !completed {
        return (review::ReviewOutcome::Incomplete, String::new());
    }
    let (verdict, summary) = review::parse_verdict(&synth);
    if !matches!(verdict, review::Verdict::Approve) {
        eprintln!(
            "[implement] review {verdict:?}:\n{}",
            verify::truncate_output(&synth, rcfg.max_output_bytes)
        );
    }
    (
        review::ReviewOutcome::Ran {
            verdict,
            summary,
            reviewers_failed,
        },
        synth,
    )
}
```

- [ ] **Step 4: Build (the loop wiring comes in Task 6)**

Run: `cargo build -p a2a-bridge 2>&1 | tail -20`
Expected: compiles with `dead_code` warnings for the three new helpers (they're wired in Task 6). That's OK
for this step.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "implement: extract total drain_impl/run_verify_step/run_review_step (b2b3b)"
```

---

## Task 6: the review→tweak loop in `implement_cmd`

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1: Resolve `loop_cfg` + `fix_graph` pre-commit**

After the `verify_cfg`/`review_cfg` capture (~line 540, before `cfg.into_snapshot()`), add:

```rust
    // B2b-3b: resolve the loop config PRE-clone-work so a malformed [implement] fails loud (nothing is
    // committed yet). Absent → LoopConfig::default() (loop ON, max_attempts=3).
    let loop_cfg = cfg
        .implement
        .as_ref()
        .map(|t| t.to_config())
        .transpose()
        .map_err(|e| format!("implement: [implement] config: {e}"))?
        .unwrap_or_default();
    // Resolve the fix workflow against the loaded map: absent → FixUnavailable (soft; no fix turns).
    let fix_graph = wf_map.get(&loop_cfg.fix_workflow).cloned();
```

- [ ] **Step 2: Use `drain_impl` for the first edit turn**

Replace the first-edit drain (lines ~563-591, from `use bridge_workflow::executor::{WorkflowEvent, …};`
through `drop(stream);`) with:

```rust
    let completed = drain_impl(executor.run_with_context(
        graph,
        a.task.clone(),
        run_id,
        tokio_util::sync::CancellationToken::new(),
        ctx,
    ))
    .await;
```

(Remove the now-unused `use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};` /
`use futures::StreamExt;` lines from this block if they aren't used elsewhere in the fn — `drain_impl` owns
them now. The build will flag any leftover unused import.)

- [ ] **Step 3: Replace the `Action::Commit` arm with the loop**

Replace the entire `implement::Action::Commit(message) => { … }` arm (lines ~624-740) with:

```rust
        implement::Action::Commit(message) => {
            // Phase 1 → 2 boundary: the ONLY post-commit `?`. After this, the body is lossy (no `?`/panic).
            let mut sha = implement::host_commit(&clone, &message)?;
            let _ = std::fs::remove_file(clone.join(".git").join("A2A_COMMIT_MSG")); // R13 hygiene
            let original_message = message; // amends keep this; the hand-off subject = its first line
            let max = loop_cfg.max_attempts;
            let mut attempt: u32 = 1;
            // Sentinels: defined even if the very first reset fails before any step runs.
            let mut last_verify = verify::VerifyOutcome::Incomplete;
            let mut last_review: (review::ReviewOutcome, String) =
                (review::ReviewOutcome::Incomplete, String::new());

            let report = loop {
                // Verify the COMMITTED tree (discard the agent's unstaged scratch first).
                if let Err(e) = implement::reset_worktree_to_head(&clone) {
                    break tweak::LoopReport {
                        attempts: attempt,
                        stop_reason: tweak::StopReason::StepError(e),
                    };
                }
                last_verify = run_verify_step(&verify_cfg, &clone_cwd, &a.repo);
                last_review = run_review_step(
                    &review_cfg, &wf_map, &executor, &a.task, &base_sha, &sha, &clone_cwd, &task_id,
                    attempt,
                )
                .await;

                match tweak::classify(attempt, max, &last_verify, &last_review.0) {
                    tweak::LoopStep::Stop(reason) => {
                        break tweak::LoopReport { attempts: attempt, stop_reason: reason }
                    }
                    tweak::LoopStep::Continue => {
                        let Some(fix_graph) = fix_graph.clone() else {
                            break tweak::LoopReport {
                                attempts: attempt,
                                stop_reason: tweak::StopReason::FixUnavailable,
                            };
                        };
                        let pre_i = match implement::head_sha(&clone) {
                            Ok(s) => s,
                            Err(e) => {
                                break tweak::LoopReport {
                                    attempts: attempt,
                                    stop_reason: tweak::StopReason::StepError(e),
                                }
                            }
                        };
                        let _ = std::fs::remove_file(clone.join(".git").join("A2A_COMMIT_MSG"));
                        // Fix context: the verify gate-failure digest + (REJECT only) the hoisted synth body.
                        let digest = match &last_verify {
                            verify::VerifyOutcome::Ran(v) => verify::failure_digest(v, 8 * 1024),
                            _ => String::new(),
                        };
                        let findings = matches!(
                            last_review.0,
                            review::ReviewOutcome::Ran { verdict: review::Verdict::Reject, .. }
                        )
                        .then(|| last_review.1.as_str());
                        let input = tweak::build_fix_input(&a.task, &digest, findings, 12 * 1024);

                        let completed = drain_impl(executor.run_with_context(
                            fix_graph,
                            input,
                            format!("impl-fix-{task_id}-{attempt}"),
                            tokio_util::sync::CancellationToken::new(),
                            bridge_workflow::executor::WorkflowRunContext {
                                session_cwd: Some(clone_cwd.clone()),
                            },
                        ))
                        .await;

                        let guard = implement::head_guard(&clone, &branch, &pre_i);
                        let stage = match implement::stage_state(&clone) {
                            Ok(s) => s,
                            Err(e) => {
                                break tweak::LoopReport {
                                    attempts: attempt,
                                    stop_reason: tweak::StopReason::StepError(e),
                                }
                            }
                        };
                        let action =
                            implement::decide(completed, guard, stage, (original_message.clone(), false));
                        match tweak::fix_step(&action) {
                            tweak::FixDisposition::Amend => match implement::host_amend_commit(&clone) {
                                Ok(s) => {
                                    sha = s;
                                    attempt += 1;
                                    continue;
                                }
                                Err(_) => {
                                    break tweak::LoopReport {
                                        attempts: attempt,
                                        stop_reason: tweak::StopReason::AmendFailed,
                                    }
                                }
                            },
                            tweak::FixDisposition::Diverged => {
                                // The agent self-committed/switched: restore the trusted cumulative tip so
                                // the hand-off's cherry-pick applies the WHOLE change, not the rogue delta.
                                let _ = implement::reset_hard(&clone, &sha);
                                break tweak::LoopReport {
                                    attempts: attempt,
                                    stop_reason: tweak::StopReason::HeadMutated,
                                };
                            }
                            tweak::FixDisposition::NoProgress => {
                                break tweak::LoopReport {
                                    attempts: attempt,
                                    stop_reason: tweak::StopReason::NoProgress,
                                }
                            }
                        }
                    }
                }
            };

            // Hand-off: built AFTER the loop with the FINAL sha + the ORIGINAL subject (patches the stale
            // committed line); then the verify + review + loop suffixes. Always prints (the invariant).
            let subject = original_message.lines().next().unwrap_or("").to_string();
            let mut handoff = implement::handoff_text(
                &clone.to_string_lossy(),
                &branch,
                &sha,
                &subject,
                &a.repo.to_string_lossy(),
            );
            handoff.push('\n');
            handoff.push_str(&verify::outcome_suffix(&last_verify));
            handoff.push('\n');
            handoff.push_str(&review::outcome_suffix(&last_review.0));
            handoff.push('\n');
            handoff.push_str(&tweak::loop_outcome_suffix(&report));
            println!("{handoff}");
            Ok(())
        }
```

- [ ] **Step 4: Build + full bin tests + clippy**

Run:
```bash
cargo build -p a2a-bridge 2>&1 | tail -20
cargo test -p a2a-bridge --bin a2a-bridge 2>&1 | tail -25
cargo clippy -p a2a-bridge --all-targets --all-features -- -D warnings 2>&1 | tail -20
```
Expected: build clean (no dead-code/unused-import warnings), all bin tests PASS, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "implement: wire the bounded review->tweak loop (b2b3b)"
```

---

## Task 7: `implement-fix` prompt + workflow + `[implement]` example

**Files:**
- Create: `prompts/implement-fix.md`
- Modify: `examples/a2a-bridge.containerized.toml`

- [ ] **Step 1: Write the prompt**

Create `prompts/implement-fix.md`:

```markdown
You are a coding agent working INSIDE a writable git clone (your current working directory) that ALREADY
contains your prior commit for this task. A build/test verify and/or a code review found problems. CONTINUE
the work and FIX them.

CONTRACT — follow exactly:
- Address every issue listed below by editing/creating files in this clone.
- STAGE exactly the files that belong in the fix with `git add <paths>` (include new files). Do NOT stage
  scratch/debug files.
- Do NOT run `git commit`. Do NOT write a commit message (the bridge keeps the original one). Do NOT switch
  branches or run `git checkout` / `git reset`. The bridge folds your staged fix into the existing commit.
- When done, STOP. Your reply text is not used.

ISSUES TO FIX:
{{input}}
```

- [ ] **Step 2: Add the `[implement]` block + `implement-fix` workflow to the example**

In `examples/a2a-bridge.containerized.toml`, after the `[review]` block (~line 22), add:

```toml
# B2b-3b: the review→tweak loop. After the first commit, the bridge runs verify+review each attempt; on a
# verify-FAIL or review-REJECT it re-prompts the `impl` agent (the `implement-fix` workflow) to fix on the
# SAME clone, then AMENDs the fix into the single commit. Bounded by max_attempts (commit turns: the initial
# commit + up to max_attempts-1 fix amends). Advisory — the operator still accepts at merge. Absent → 3.
[implement]
max_attempts = 3
fix_workflow = "implement-fix"
```

Then add the workflow beside `implement-edit` (after its block, ~line 202):

```toml
# ── implement-fix (B2b-3b): one ContainerRw `impl` turn to FIX verify/review failures on the existing
#    clone. EXAMPLE-ONLY (references `impl`, which is not in the `init` scaffold). ──
[[workflows]]
id = "implement-fix"
[[workflows.nodes]]
id = "fix"
agent = "impl"
prompt_file = "../prompts/implement-fix.md"
inputs = []
```

- [ ] **Step 3: Verify the example still loads (extend the existing R11 test)**

The test at `main.rs` ~line 2014 loads the containerized example and checks `implement-edit`. Add an
assertion in that same test that `implement-fix` loads:

```rust
        assert!(
            wf.contains_key(&bridge_core::ids::WorkflowId::parse("implement-fix").unwrap()),
            "implement-fix workflow loads"
        );
```

Run: `cargo test -p a2a-bridge --bin a2a-bridge 2>&1 | tail -15`
Expected: PASS (the `init_generated_config_parses_and_loads` count stays 5 — `implement-fix` is
example-only, not in `INIT_WORKFLOWS`).

- [ ] **Step 4: Commit**

```bash
git add prompts/implement-fix.md examples/a2a-bridge.containerized.toml bin/a2a-bridge/src/main.rs
git commit -m "implement-fix: prompt + example workflow + [implement] block (b2b3b)"
```

---

## Task 8: workspace gate + coverage

**Files:** none (verification only)

- [ ] **Step 1: fmt + full workspace build/test/clippy**

Run (serialized — do NOT parallelize heavy cargo jobs):
```bash
cargo fmt --all
cargo build --workspace 2>&1 | tail -15
cargo test --workspace --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill 2>&1 | tail -25
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -20
```
Expected: all green. (bridge-container + the two process tests need host capabilities; CI runs the full
suite — mirror the example `[verify]` exclusions.)

- [ ] **Step 2: Coverage**

Run:
```bash
cargo llvm-cov clean --workspace
cargo llvm-cov --workspace --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill 2>&1 | tail -30
```
Expected: workspace ≥ 85; bridge-core/acp/api/workflow ≥ 90. The new pure helpers (`tweak.rs`,
`verify::failure_digest`, `config::ImplementToml::to_config`) should sit ≥ 95 (they're fully unit-tested).
If a pure helper is under-covered, add a unit test — do NOT lower a floor.

- [ ] **Step 3: Commit any fmt-only changes**

```bash
git add -A
git commit -m "chore: cargo fmt (b2b3b)" || echo "nothing to format"
```

---

## Task 9: live gate (operator-run; Docker)

**Files:** none (manual validation; pause for Wesley to run the Docker-dependent steps)

Prereqs: the egress proxies up, the `a2a-toolchain` + reader images built, creds in place (same as B2b-2/3a).
Use a throwaway clone of THIS repo under `allowed_cwd_root`. Serialize heavy Docker jobs.

- [ ] **Step 1: Right-first-try (1 attempt)**

A trivially-correct task (e.g. "add a top-level `SCRATCH.md` with one line"). Run:
```bash
a2a-bridge implement "add a top-level SCRATCH.md saying hello" \
  --repo <throwaway-clone> --config examples/a2a-bridge.containerized.toml
```
Expected hand-off: `committed <sha> "..."`, `verify: PASS …`, `review: APPROVE …`, `loop: 1 attempt(s) —
converged`. EXACTLY ONE commit on the branch; `cherry-pick -n FETCH_HEAD` applies the change.

- [ ] **Step 2: Verify-fail → fix → pass (≥2 attempts, still one commit)**

A task that the agent first gets slightly wrong on a lint/test (or inject a `cargo fmt` violation in the
edit). Expect: attempt 1 `verify: FAIL`, a fix turn runs (`node fix started`), attempt 2 `verify: PASS` +
`review: APPROVE`, `loop: 2 attempt(s) — converged`, and STILL one amended commit (`rev-list --count
base..HEAD == 1`).

- [ ] **Step 3: Bound reached (`max_attempts=1`)**

Run with a config whose `[implement].max_attempts = 1` against a task that fails verify. Expect: one commit,
`verify: FAIL …`, `loop: 1 attempt(s) — bound reached`, **exit 0**, clone left for the operator.

- [ ] **Step 4: Reaper holds**

During/after a multi-attempt run: `docker ps -a --filter name=a2a-` shows the `:rw` fix container + the
`:ro` review containers reaped (→ 0 within ~2s of each turn). Confirms the fresh-token-per-iteration
cancel-drain didn't regress the reaper.

- [ ] **Step 5: Record results**

Capture the hand-off blocks + `rev-list --count` + `docker ps -a` for the ADR. The dangerous paths
(`HeadMutated`/`NoProgress`/`AmendFailed`/`NotActionable`) are covered by the pure `classify`/`fix_step`
tests + the temp-repo `reset_hard`/amend tests (a live agent won't reliably self-commit on demand —
mirroring B2b-3a, where the live REJECT was impractical and unit-tested instead).

---

## After the build

Per the established cadence: **plan dual-review** (containerized dogfood PRIMARY via `plan-review` +
a2a-local codex backstop with `--agent codex-review`) BEFORE building; fold into a plan rev2 if needed.
Then inline TDD build (Tasks 1–8), live gate (Task 9), merge + push, memory, **ADR-0023**.

## Self-review (writing-plans)

- **Spec coverage:** silent-work-loss→`HeadMutated`+`reset_hard` (T4+T6); config lifecycle (T2+T6 pre-commit
  resolve; FixUnavailable in T6); total helpers + per-iteration token (T5); verify the committed tree (T4
  `reset_worktree_to_head` + T6 top-of-loop); amend keeps original message (T4); classify reviewers_failed
  cases (T3); `build_fix_input`/`failure_digest` formats (T1+T3); implement-fix example-only + init count 5
  (T7); attempt-qualified review id (T5); sentinels (T6); final-sha hand-off patch (T6). All covered.
- **Type consistency:** `VerifyOutcome::Incomplete` added (T1) before use (T3/T6); `LoopConfig`/
  `ImplementToml::to_config` (T2) match the T6 call; `tweak::*` signatures match the T6 call site;
  `fix_step` consumes `implement::Action` (derives `PartialEq, Eq` already); `run_review_step` returns
  `(ReviewOutcome, String)` consumed as `last_review.0`/`.1`.
- **No placeholders:** every code step is complete.
