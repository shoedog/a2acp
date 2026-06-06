# B2b-3b — Review→Tweak Loop — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make `a2a-bridge implement` self-correcting: after the first commit, run a bounded
verify→review→classify→fix→amend loop on the SAME clone until APPROVE+PASS or `[implement].max_attempts`,
then hand off the best-effort branch + the final state. Advisory (exit 0).

**Architecture:** A pure decision core (`tweak.rs`: `classify`/`fix_step`/`build_fix_input`/
`loop_outcome_suffix`) plus an **injectable loop** `run_tweak_loop(clone, …, &mut dyn TweakEffects)` whose
git ops run against a REAL clone (temp-repo testable) and whose workflow effects (verify/review/fix) are
behind a `TweakEffects` seam — so the no-work-loss wiring is unit-tested with a FAKE executor. Production wires
a `ProdEffects` impl in `implement_cmd`. The first commit stays fail-loud (phase 1); everything after is lossy
(phase 2 — no `?`/panic; every fallible op → a `StopReason`). Each fix AMENDs into the single commit (parent
stays `base_sha`, original message kept) so the hand-off is byte-identical. A fix-turn that mutates HEAD →
`restore_branch(branch, last_good_sha)` (robust to a branch-switch) → no work loss.

**Tech Stack:** Rust (workspace), `bin/a2a-bridge` (config.rs, implement.rs, verify.rs, review.rs, the new
tweak.rs, main.rs), `bridge_workflow::executor`, the ContainerRw `impl` agent, Docker (live gate only).

**Spec:** `docs/superpowers/specs/2026-06-06-review-tweak-loop-b2b3b-design.md` (rev3, dual-reviewed). This
plan REFINES the spec on two points surfaced by the plan dual-review: (a) `reset_hard(clone, sha)` →
`restore_branch(clone, branch, sha)` (the hand-off fetches the BRANCH ref, which a bare `reset --hard` leaves
at the rogue tip if the agent switched branches); (b) two new `StopReason`s — `RestoreFailed` (untrusted
branch after a failed restore) and `FixIncomplete` (the fix workflow didn't complete — distinct from a HEAD
mutation). The spec's intent (no work loss, distinct stop reason, fake-executor-testable) is preserved.

**Conventions:** TDD green-per-task (red step first); task/code commits do NOT carry the `Co-Authored-By`
trailer (doc commits do). Coverage after `cargo llvm-cov clean --workspace`; floors per **ci.yml** (workspace
85, bridge-core/acp/api/workflow 90 — the bin crate has no per-crate floor; keep `tweak.rs`/`verify.rs`/
`config.rs` pure+loop helpers ≥95 and pin the `classify` cross-product cells explicitly — line coverage alone
won't). Branch `feat/implement-b2b3b` off `main`.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `bin/a2a-bridge/src/tweak.rs` | **CREATE** | Pure core (`StopReason`/`LoopStep`/`LoopReport`/`FixDisposition`, `classify`, `fix_step`, `build_fix_input`, `loop_outcome_suffix`) + the injectable `run_tweak_loop` + the `TweakEffects` seam (+ its fake-executor tests). |
| `bin/a2a-bridge/src/verify.rs` | modify | `VerifyOutcome::Incomplete` + its `outcome_suffix` arm; pure `failure_digest`. |
| `bin/a2a-bridge/src/config.rs` | modify | `ImplementToml`/`LoopConfig`(+`Default`)/`to_config` + `RegistryConfig.implement`. |
| `bin/a2a-bridge/src/implement.rs` | modify | `host_commit_argv_run`; `commit_amend_argv`/`host_amend_commit`; `reset_worktree_to_head`; `restore_branch`. |
| `bin/a2a-bridge/src/main.rs` | modify | `mod tweak;`; total `drain_impl`/`run_verify_step`/`run_review_step`; `ProdEffects` impl `TweakEffects`; pre-clone `loop_cfg`; call `run_tweak_loop`; hand-off from `LoopFinal`. |
| `prompts/implement-fix.md` | **CREATE** | The fix-turn prompt (continue & fix; re-stage; don't commit; no message). |
| `examples/a2a-bridge.containerized.toml` | modify | `[implement]` block + the `implement-fix` workflow (example-only). |

---

## Task 1: `VerifyOutcome::Incomplete` + `failure_digest` (pure, verify.rs)

**Files:** Modify + test `bin/a2a-bridge/src/verify.rs`

- [ ] **Step 1: Write the failing tests**

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
        let v = aggregate(vec![
            VerifyResult { name: "fmt".into(), gate: true, ok: true, output: "ok".into() },
            VerifyResult { name: "clippy".into(), gate: true, ok: false, output: "E".repeat(50) },
        ]);
        let d = failure_digest(&v, 20);
        assert!(d.contains("### clippy"));
        assert!(!d.contains("### fmt"));
        assert!(d.contains("truncated"));
    }

    #[test]
    fn failure_digest_empty_when_no_gate_failures() {
        let v = aggregate(vec![
            VerifyResult { name: "test".into(), gate: true, ok: true, output: "ok".into() },
            VerifyResult { name: "cov".into(), gate: false, ok: false, output: "x".into() },
        ]);
        assert_eq!(failure_digest(&v, 4096), "");
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
Expected: FAIL — `VerifyOutcome::Incomplete` / `failure_digest` don't exist.

- [ ] **Step 3: Implement**

Add the `Incomplete` variant after `ConfigError`:

```rust
    /// The step did not run to completion (e.g. a pre-verify worktree reset failed) — the loop sentinel +
    /// catch-all so the always-print hand-off has a defined value. (B2b-3b.)
    Incomplete,
```

Add the `outcome_suffix` arm:

```rust
        VerifyOutcome::Incomplete => "verify: incomplete (did not finish)".to_string(),
```

Add `failure_digest` after `truncate_output`:

```rust
/// PURE. The fix-turn digest: ONLY the GATE failures (the ones that fail the verdict + drive `actionable`),
/// in order, each `### <name>` + its (truncated) output. Non-gate failures are reported in the hand-off but
/// never re-prompted. Empty when no gate failed. `run_verify` stops at the first gate failure, so this is
/// normally one entry; the per-result budget splits `max_bytes` across however many there are.
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
Expected: PASS (incl. the existing `outcome_suffix_covers_three_arms` — it doesn't assert exhaustiveness).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/verify.rs
git commit -m "verify: add VerifyOutcome::Incomplete + pure failure_digest (b2b3b)"
```

---

## Task 2: `[implement]` config — `ImplementToml`/`LoopConfig` (config.rs)

**Files:** Modify + test `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn implement_config_defaults_when_absent() {
        let lc = ImplementToml { max_attempts: None, fix_workflow: None }.to_config().unwrap();
        assert_eq!(lc.max_attempts, 3);
        assert_eq!(lc.fix_workflow.as_str(), "implement-fix");
        assert_eq!(LoopConfig::default().max_attempts, 3);
        assert_eq!(LoopConfig::default().fix_workflow.as_str(), "implement-fix");
    }

    #[test]
    fn implement_config_max_attempts_zero_is_error() {
        assert!(ImplementToml { max_attempts: Some(0), fix_workflow: None }.to_config().is_err());
    }

    #[test]
    fn implement_config_clamps_above_hard_max() {
        let lc = ImplementToml { max_attempts: Some(99), fix_workflow: None }.to_config().unwrap();
        assert_eq!(lc.max_attempts, 10);
    }

    #[test]
    fn implement_config_custom_fix_workflow_and_malformed() {
        let lc = ImplementToml { max_attempts: Some(2), fix_workflow: Some("my-fix".into()) }
            .to_config().unwrap();
        assert_eq!(lc.max_attempts, 2);
        assert_eq!(lc.fix_workflow.as_str(), "my-fix");
        assert!(ImplementToml { max_attempts: None, fix_workflow: Some("".into()) }.to_config().is_err());
    }

    #[test]
    fn implement_block_parses_from_toml() {
        let c = RegistryConfig::parse(
            "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n\
             [implement]\nmax_attempts=2\nfix_workflow=\"implement-fix\"\n",
        ).unwrap();
        assert_eq!(c.implement.as_ref().unwrap().to_config().unwrap().max_attempts, 2);
        let c2 = RegistryConfig::parse(
            "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n",
        ).unwrap();
        assert!(c2.implement.is_none());
    }
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p a2a-bridge --bin a2a-bridge config::tests::implement 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement**

Add to `RegistryConfig` (after `review`):

```rust
    /// `[implement]` (Slice B2b-3b): the review→tweak loop config. Absent → `LoopConfig::default()`.
    #[serde(default)]
    pub implement: Option<ImplementToml>,
```

Add after the `ReviewToml`/`ReviewConfig` block:

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
/// `FixUnavailable`, never an abort). A malformed block is fail-loud PRE-clone (resolved before the clone).
#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub max_attempts: u32,
    pub fix_workflow: bridge_core::ids::WorkflowId,
}

fn default_fix_workflow_id() -> bridge_core::ids::WorkflowId {
    bridge_core::ids::WorkflowId::parse("implement-fix").expect("static id is valid")
}

const IMPLEMENT_HARD_MAX: u32 = 10;

impl Default for LoopConfig {
    fn default() -> Self {
        Self { max_attempts: 3, fix_workflow: default_fix_workflow_id() }
    }
}

impl ImplementToml {
    pub fn to_config(&self) -> Result<LoopConfig, ConfigError> {
        let max_attempts = match self.max_attempts {
            None => 3,
            Some(0) => return Err(ConfigError::Registry("[implement] max_attempts must be >= 1".into())),
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
        Ok(LoopConfig { max_attempts, fix_workflow })
    }
}
```

- [ ] **Step 4: Run to verify pass** — `cargo test -p a2a-bridge --bin a2a-bridge config::tests::implement 2>&1 | tail -20` → PASS.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "config: [implement] block + LoopConfig (b2b3b)"
```

---

## Task 3: pure loop core — `tweak.rs` (CREATE; red-first)

**Files:** Create `bin/a2a-bridge/src/tweak.rs`; modify `bin/a2a-bridge/src/main.rs` (`mod tweak;`).

- [ ] **Step 1: Create the module — tests + STUBS (the RED step)**

Create `bin/a2a-bridge/src/tweak.rs` with the types/signatures STUBBED with `todo!()` and the pure tests
present (so the run is genuinely red — the stubs panic):

```rust
//! The B2b-3b review→tweak loop. PURE core (`classify`/`fix_step`/`build_fix_input`/`loop_outcome_suffix`)
//! + the injectable `run_tweak_loop` driven through the `TweakEffects` seam — so the no-work-loss wiring is
//! unit-tested with a FAKE executor while the git ops run on a REAL clone. No panics, no slicing (B2b-3a's
//! em-dash lesson); phase-2 totality (no `?`).

use crate::review::{ReviewOutcome, Verdict};
use crate::verify::VerifyOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Success,
    BoundReached,
    NotActionable,
    NoProgress,         // a fix turn staged nothing new (NoCommitClean/Dirty)
    HeadMutated,        // a fix turn advanced/switched HEAD; the branch was restored to last-good
    RestoreFailed(String), // HEAD diverged AND restoring the branch failed → the branch tip is UNTRUSTED
    FixIncomplete,      // the fix workflow did not complete (NOT a HEAD mutation)
    AmendFailed,
    StepError(String),  // a post-commit git op (reset/stage/head) failed — reduced, never `?`
    FixUnavailable,     // actionable but no fix workflow is registered
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopStep { Continue, Stop(StopReason) }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopReport { pub attempts: u32, pub stop_reason: StopReason }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixDisposition { Amend, Diverged, NoProgress }

/// The final loop state for the hand-off (the report + the FINAL sha + the LAST verify/review outcomes).
#[derive(Debug)]
pub struct LoopFinal {
    pub report: LoopReport,
    pub sha: String,
    pub last_verify: VerifyOutcome,
    pub last_review: ReviewOutcome,
}

pub fn fix_step(_action: &crate::implement::Action) -> FixDisposition { todo!() }
pub fn classify(_attempt: u32, _max: u32, _v: &VerifyOutcome, _r: &ReviewOutcome) -> LoopStep { todo!() }
pub fn build_fix_input(_task: &str, _vd: &str, _rf: Option<&str>, _max: usize) -> String { todo!() }
pub fn loop_outcome_suffix(_rep: &LoopReport) -> String { todo!() }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::{aggregate, VerifyResult};

    fn ran(passed: bool) -> VerifyOutcome {
        VerifyOutcome::Ran(aggregate(vec![VerifyResult { name: "t".into(), gate: true, ok: passed, output: String::new() }]))
    }
    fn rev(v: Verdict, failed: usize) -> ReviewOutcome {
        ReviewOutcome::Ran { verdict: v, summary: "s".into(), reviewers_failed: failed }
    }

    #[test]
    fn success_when_both_ok_incl_degraded_approve() {
        assert_eq!(classify(1, 3, &ran(true), &rev(Verdict::Approve, 0)), LoopStep::Stop(StopReason::Success));
        assert_eq!(classify(1, 3, &ran(true), &rev(Verdict::Approve, 1)), LoopStep::Stop(StopReason::Success));
        assert_eq!(classify(1, 1, &VerifyOutcome::NotConfigured, &ReviewOutcome::NotConfigured),
                   LoopStep::Stop(StopReason::Success));
    }

    #[test]
    fn continue_when_actionable_under_bound() {
        assert_eq!(classify(1, 3, &ran(false), &rev(Verdict::Approve, 0)), LoopStep::Continue);
        assert_eq!(classify(1, 3, &ran(true), &rev(Verdict::Reject, 0)), LoopStep::Continue);
        assert_eq!(classify(1, 3, &ran(true), &rev(Verdict::Reject, 1)), LoopStep::Continue);
        assert_eq!(classify(1, 3, &ran(false), &ReviewOutcome::NotConfigured), LoopStep::Continue);
        // cross-product cell: verify ConfigError but review Reject → still actionable (OR), NOT vetoed.
        assert_eq!(classify(1, 3, &VerifyOutcome::ConfigError, &rev(Verdict::Reject, 0)), LoopStep::Continue);
    }

    #[test]
    fn bound_reached_at_max() {
        assert_eq!(classify(3, 3, &ran(false), &rev(Verdict::Reject, 0)), LoopStep::Stop(StopReason::BoundReached));
        assert_eq!(classify(1, 1, &ran(false), &ReviewOutcome::NotConfigured), LoopStep::Stop(StopReason::BoundReached));
    }

    #[test]
    fn not_actionable_cross_product() {
        // Inconclusive review + pass verify
        assert_eq!(classify(1, 3, &ran(true), &rev(Verdict::Inconclusive, 0)), LoopStep::Stop(StopReason::NotActionable));
        // verify Incomplete + degraded Approve (failed>0): verify not-ok but not actionable
        assert_eq!(classify(1, 3, &VerifyOutcome::Incomplete, &rev(Verdict::Approve, 1)), LoopStep::Stop(StopReason::NotActionable));
        // verify ConfigError + review Approve: not-ok verify, ok review, not actionable
        assert_eq!(classify(1, 3, &VerifyOutcome::ConfigError, &rev(Verdict::Approve, 0)), LoopStep::Stop(StopReason::NotActionable));
        // verify ConfigError + review Incomplete: neither ok nor actionable
        assert_eq!(classify(1, 3, &VerifyOutcome::ConfigError, &ReviewOutcome::Incomplete), LoopStep::Stop(StopReason::NotActionable));
        // verify NotConfigured + review NotLoaded
        assert_eq!(classify(1, 3, &VerifyOutcome::NotConfigured, &ReviewOutcome::NotLoaded), LoopStep::Stop(StopReason::NotActionable));
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
        let i = build_fix_input("do X", "### clippy\nerr", Some("BLOCKER: bug"), 4096);
        assert!(i.contains("do X") && i.contains("## Verify failures") && i.contains("### clippy"));
        assert!(i.contains("## Review findings (REJECTED)") && i.contains("BLOCKER: bug"));
        let v = build_fix_input("do X", "### test\nfail", None, 4096);
        assert!(v.contains("## Verify failures") && !v.contains("Review findings"));
        let r = build_fix_input("do X", "", Some("MAJOR: y"), 4096);
        assert!(!r.contains("## Verify failures") && r.contains("Review findings"));
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
        assert!(mk(StopReason::RestoreFailed("io".into())).contains("UNTRUSTED"));
        assert!(mk(StopReason::FixIncomplete).contains("did not complete"));
        assert!(mk(StopReason::AmendFailed).contains("amend failed"));
        assert!(mk(StopReason::FixUnavailable).contains("no fix workflow"));
        assert!(mk(StopReason::StepError("boom".into())).contains("boom"));
    }
}
```

In `main.rs` add `mod tweak;` beside `mod review;`.

- [ ] **Step 2: Run — expect RED**

Run: `cargo test -p a2a-bridge --bin a2a-bridge tweak::tests 2>&1 | tail -20`
Expected: tests FAIL (the `todo!()` stubs panic).

- [ ] **Step 3: Implement the pure fns** (replace the four `todo!()` stubs)

```rust
pub fn fix_step(action: &crate::implement::Action) -> FixDisposition {
    use crate::implement::Action;
    match action {
        Action::Commit(_) => FixDisposition::Amend,
        Action::Abort(_) => FixDisposition::Diverged,
        Action::NoCommitClean | Action::NoCommitDirty => FixDisposition::NoProgress,
    }
}

pub fn classify(attempt: u32, max_attempts: u32, v: &VerifyOutcome, r: &ReviewOutcome) -> LoopStep {
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
    let review_actionable = matches!(r, ReviewOutcome::Ran { verdict: Verdict::Reject, .. });
    if !(verify_actionable || review_actionable) {
        return LoopStep::Stop(StopReason::NotActionable);
    }
    if attempt >= max_attempts {
        return LoopStep::Stop(StopReason::BoundReached);
    }
    LoopStep::Continue
}

pub fn build_fix_input(task: &str, verify_digest: &str, review_findings: Option<&str>, max_bytes: usize) -> String {
    let header = format!(
        "{task}\n\nThe previous attempt did not pass. FIX the issues below on the current clone (it already \
         has your prior commit); re-stage your fixes with `git add`; do NOT run `git commit` and do NOT write \
         a commit message.\n"
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

pub fn loop_outcome_suffix(rep: &LoopReport) -> String {
    let why = match &rep.stop_reason {
        StopReason::Success => "converged".to_string(),
        StopReason::BoundReached => "bound reached".to_string(),
        StopReason::NotActionable => "no actionable signal".to_string(),
        StopReason::NoProgress => "fix turn staged nothing".to_string(),
        StopReason::HeadMutated => "fix turn diverged HEAD — reset to last-good".to_string(),
        StopReason::RestoreFailed(e) => format!("fix turn diverged HEAD and the branch is UNTRUSTED (restore failed: {e}) — inspect the clone; do NOT use the merge command above"),
        StopReason::FixIncomplete => "fix turn did not complete".to_string(),
        StopReason::AmendFailed => "amend failed".to_string(),
        StopReason::FixUnavailable => "no fix workflow configured".to_string(),
        StopReason::StepError(e) => format!("step error: {e}"),
    };
    format!("loop: {} attempt(s) — {}", rep.attempts, why)
}
```

- [ ] **Step 4: Run to verify pass** — `cargo test -p a2a-bridge --bin a2a-bridge tweak::tests 2>&1 | tail -20` → PASS.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/tweak.rs bin/a2a-bridge/src/main.rs
git commit -m "tweak: pure loop core (classify/fix_step/build_fix_input/suffix) (b2b3b)"
```

---

## Task 4: amend + reset + restore git ops (implement.rs, temp-repo)

**Files:** Modify + test `bin/a2a-bridge/src/implement.rs`

- [ ] **Step 1: Write the failing tests** (append to the temp-repo test section)

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
        std::fs::write(p.join("B.md"), "b\n").unwrap();
        run_git(Some(&p), &["add", "B.md"]).unwrap();
        let sha2 = host_amend_commit(&p).unwrap();
        assert_ne!(sha1, sha2);
        let count = run_git(Some(&p), &["rev-list", "--count", &format!("{base}..HEAD")]).unwrap();
        assert_eq!(String::from_utf8_lossy(&count.stdout).trim(), "1");
        let parent = run_git(Some(&p), &["rev-parse", "HEAD^"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&parent.stdout).trim(), base);
        let subj = run_git(Some(&p), &["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&subj.stdout).trim(), "feat: the change");
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
        std::fs::write(p.join("A.md"), "MUTATED\n").unwrap();
        std::fs::write(p.join("scratch.tmp"), "junk\n").unwrap();
        assert_ne!(stage_state(&p).unwrap(), StageState::Clean);
        reset_worktree_to_head(&p).unwrap();
        assert_eq!(stage_state(&p).unwrap(), StageState::Clean);
        assert_eq!(std::fs::read_to_string(p.join("A.md")).unwrap(), "a\n");
        assert!(!p.join("scratch.tmp").exists());
    }

    #[test]
    fn restore_branch_recovers_after_head_advance() {
        let (_g, p) = temp_repo();
        run_git(Some(&p), &["checkout", "-q", "-b", "implement/x"]).unwrap();
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        run_git(Some(&p), &["add", "A.md"]).unwrap();
        let good = host_commit(&p, "feat").unwrap();
        // agent self-commits on the SAME branch
        std::fs::write(p.join("rogue.md"), "r\n").unwrap();
        run_git(Some(&p), &["add", "rogue.md"]).unwrap();
        run_git(Some(&p), &["commit", "-q", "-m", "rogue"]).unwrap();
        restore_branch(&p, "implement/x", &good).unwrap();
        assert_eq!(head_sha(&p).unwrap(), good);
        assert_eq!(current_branch(&p).unwrap(), "implement/x");
        assert!(p.join("A.md").exists() && !p.join("rogue.md").exists());
    }

    #[test]
    fn restore_branch_recovers_after_branch_switch() {
        // the deeper hole: a bare `reset --hard` would reset the WRONG branch. restore_branch must force
        // OUR branch (the one the hand-off fetches) back to the trusted tip.
        let (_g, p) = temp_repo();
        run_git(Some(&p), &["checkout", "-q", "-b", "implement/x"]).unwrap();
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        run_git(Some(&p), &["add", "A.md"]).unwrap();
        let good = host_commit(&p, "feat").unwrap();
        // agent SWITCHES to a new branch and commits there
        run_git(Some(&p), &["checkout", "-q", "-b", "rogue-branch"]).unwrap();
        std::fs::write(p.join("rogue.md"), "r\n").unwrap();
        run_git(Some(&p), &["add", "rogue.md"]).unwrap();
        run_git(Some(&p), &["commit", "-q", "-m", "rogue"]).unwrap();
        restore_branch(&p, "implement/x", &good).unwrap();
        // OUR branch must be back at the trusted tip (what the hand-off fetches).
        assert_eq!(current_branch(&p).unwrap(), "implement/x");
        let tip = run_git(Some(&p), &["rev-parse", "implement/x"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&tip.stdout).trim(), good);
        assert!(p.join("A.md").exists() && !p.join("rogue.md").exists());
    }
```

- [ ] **Step 2: Run to verify fail** — `cargo test -p a2a-bridge --bin a2a-bridge implement::tests::restore_branch 2>&1 | tail -20` → FAIL.

- [ ] **Step 3: Implement** — refactor `host_commit` to share a runner; add the new fns. Replace the existing
`host_commit` (lines ~224-248) with:

```rust
/// Run a prepared commit argv with `git -C <clone>`: the bounded index-lock retry, the stale-`.git/index.
/// lock` clear after retries, and reading the new sha. Shared by `host_commit` + `host_amend_commit`.
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
    let _ = std::fs::remove_file(clone.join(".git").join("index.lock"));
    let out = run_git(Some(clone), &refs).map_err(|e| format!("git commit: {e}"))?;
    if out.status.success() {
        head_sha(clone)
    } else {
        Err(format!("git commit failed after lock retries: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Deterministically commit the AGENT-STAGED index with the bot identity + the full hook/sign/ownership pins.
pub fn host_commit(clone: &Path, msg: &str) -> Result<String, String> {
    host_commit_argv_run(clone, &commit_argv(&clone.to_string_lossy(), msg))
}

/// `commit --amend --no-edit` argv with the SAME pins (run with `git -C <clone>`). Folds the freshly-staged
/// fix into the single commit, KEEPING the stored message + parent — so the hand-off stays byte-identical.
pub fn commit_amend_argv(clone: &str) -> Vec<String> {
    vec![
        "-c".into(), format!("safe.directory={clone}"),
        "-c".into(), "core.hooksPath=/dev/null".into(),
        "-c".into(), "commit.gpgsign=false".into(),
        "-c".into(), format!("user.name={BOT_NAME}"),
        "-c".into(), format!("user.email={BOT_EMAIL}"),
        "commit".into(), "--no-verify".into(), "--amend".into(), "--no-edit".into(),
    ]
}

pub fn host_amend_commit(clone: &Path) -> Result<String, String> {
    host_commit_argv_run(clone, &commit_amend_argv(&clone.to_string_lossy()))
}

/// Reset the working tree to the committed HEAD (discard unstaged tracked changes + untracked files) so VERIFY
/// tests EXACTLY the committed tree, not the agent's leftover scratch.
pub fn reset_worktree_to_head(clone: &Path) -> Result<(), String> {
    let sd = format!("safe.directory={}", clone.to_string_lossy());
    git_ok(Some(clone), &["-c", &sd, "reset", "--hard", "HEAD"])?;
    git_ok(Some(clone), &["-c", &sd, "clean", "-fdq"]).map(|_| ())
}

/// Restore OUR task branch to a trusted commit after a fix turn mutated HEAD (advanced OR switched branches).
/// `checkout -f <branch>` returns to our branch (robust to a switch; discards the rogue working tree), then
/// `reset --hard <sha>` moves the branch ref to the trusted tip — which is what the hand-off FETCHES. This is
/// the no-work-loss fix: a bare `reset --hard` on the agent's (possibly switched) HEAD would leave OUR branch
/// at the rogue tip.
pub fn restore_branch(clone: &Path, branch: &str, sha: &str) -> Result<(), String> {
    let sd = format!("safe.directory={}", clone.to_string_lossy());
    git_ok(Some(clone), &["-c", &sd, "checkout", "-q", "-f", branch])?;
    git_ok(Some(clone), &["-c", &sd, "reset", "--hard", sha]).map(|_| ())
}
```

- [ ] **Step 4: Run to verify pass** — `cargo test -p a2a-bridge --bin a2a-bridge implement:: 2>&1 | tail -25` → PASS (incl. the existing `host_commit_pins_neutralize_all_hooks…`).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/implement.rs
git commit -m "implement: amend + reset-to-head + restore-branch git ops (b2b3b)"
```

---

## Task 5: the injectable loop — `run_tweak_loop` + `TweakEffects` + fake-executor tests (tweak.rs)

This is the spec-mandated seam: the loop's no-work-loss wiring (trusted-sha restore, sentinel init,
`attempt += 1`, no-`?` discipline, `completed`-vs-`Abort` disambiguation) is unit-tested with a FAKE executor
against a REAL git clone.

**Files:** Modify + test `bin/a2a-bridge/src/tweak.rs`. Add `async-trait` to `bin/a2a-bridge/Cargo.toml`
`[dependencies]` if not already present (the crate already uses it elsewhere — confirm with
`grep async-trait bin/a2a-bridge/Cargo.toml`; add `async-trait = "0.1"` if missing).

- [ ] **Step 1: Write the failing integration tests** (append to `tweak.rs` tests)

```rust
    use crate::implement;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn git(p: &Path, args: &[&str]) {
        assert!(Command::new("git").arg("-C").arg(p).args(args).status().unwrap().success(), "git {:?}", args);
    }

    /// A temp repo with a base commit, on branch implement/x with ONE implement commit (A.md). Returns
    /// (guard, clone_path, base_sha, sha0).
    fn loop_repo() -> (tempfile::TempDir, PathBuf, String, String) {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().to_path_buf();
        git(&p, &["init", "-q", "-b", "main"]);
        git(&p, &["config", "user.name", "t"]);
        git(&p, &["config", "user.email", "t@t"]);
        std::fs::write(p.join("README.md"), "hi\n").unwrap();
        git(&p, &["add", "README.md"]);
        git(&p, &["commit", "-q", "-m", "base"]);
        let base = implement::head_sha(&p).unwrap();
        git(&p, &["checkout", "-q", "-b", "implement/x"]);
        std::fs::write(p.join("A.md"), "a\n").unwrap();
        git(&p, &["add", "A.md"]);
        let sha0 = implement::host_commit(&p, "feat").unwrap();
        (td, p, base, sha0)
    }

    #[derive(Clone)]
    enum FixAct { Stage(&'static str), Nothing, SelfCommit(&'static str), SwitchCommit(&'static str), Incomplete }

    struct Fake {
        clone: PathBuf,
        verify: Vec<VerifyOutcome>,
        review: Vec<ReviewOutcome>,
        fixes: Vec<FixAct>,
    }
    fn at<T: Clone>(v: &[T], i: u32) -> T { v[((i as usize).saturating_sub(1)).min(v.len() - 1)].clone() }

    #[async_trait::async_trait]
    impl TweakEffects for Fake {
        async fn verify(&mut self, attempt: u32) -> VerifyOutcome { at(&self.verify, attempt) }
        async fn review(&mut self, attempt: u32, _head: &str) -> (ReviewOutcome, String) {
            (at(&self.review, attempt), "BLOCKER: synth body".into())
        }
        async fn fix(&mut self, attempt: u32, _input: &str) -> bool {
            match at(&self.fixes, attempt) {
                FixAct::Stage(f) => { std::fs::write(self.clone.join(f), "x\n").unwrap(); git(&self.clone, &["add", f]); true }
                FixAct::Nothing => true,
                FixAct::SelfCommit(f) => { std::fs::write(self.clone.join(f), "x\n").unwrap(); git(&self.clone, &["add", f]); git(&self.clone, &["commit", "-q", "-m", "rogue"]); true }
                FixAct::SwitchCommit(f) => { git(&self.clone, &["checkout", "-q", "-b", "rogue-b"]); std::fs::write(self.clone.join(f), "x\n").unwrap(); git(&self.clone, &["add", f]); git(&self.clone, &["commit", "-q", "-m", "rogue"]); true }
                FixAct::Incomplete => false,
            }
        }
    }

    fn ahead(p: &Path, base: &str) -> usize {
        let o = implement::run_git(Some(p), &["rev-list", "--count", &format!("{base}..HEAD")]).unwrap();
        String::from_utf8_lossy(&o.stdout).trim().parse().unwrap()
    }

    #[tokio::test]
    async fn loop_reject_then_approve_amends_one_commit() {
        let (_g, p, base, sha0) = loop_repo();
        let mut fake = Fake {
            clone: p.clone(),
            verify: vec![ran_pass()],
            review: vec![rev(Verdict::Reject, 0), rev(Verdict::Approve, 0)],
            fixes: vec![FixAct::Stage("B.md")],
        };
        let f = run_tweak_loop(&p, "implement/x", "task", sha0, "feat", 3, true, &mut fake).await;
        assert_eq!(f.report.stop_reason, StopReason::Success);
        assert_eq!(f.report.attempts, 2);
        assert_eq!(ahead(&p, &base), 1); // amended, still one commit
        assert!(p.join("A.md").exists() && p.join("B.md").exists());
    }

    #[tokio::test]
    async fn loop_self_commit_after_amend_preserves_cumulative_tree() {
        // THE no-work-loss test: attempt 1 stages B (amended in); attempt 2 the agent ROGUE self-commits.
        // restore_branch must leave the branch at the AMENDED tip (A+B), NOT the rogue delta.
        let (_g, p, base, sha0) = loop_repo();
        let mut fake = Fake {
            clone: p.clone(),
            verify: vec![ran_fail()], // always actionable
            review: vec![rev(Verdict::Approve, 0)],
            fixes: vec![FixAct::Stage("B.md"), FixAct::SelfCommit("rogue.md")],
        };
        let f = run_tweak_loop(&p, "implement/x", "task", sha0, "feat", 3, true, &mut fake).await;
        assert_eq!(f.report.stop_reason, StopReason::HeadMutated);
        assert_eq!(f.report.attempts, 2);
        assert_eq!(ahead(&p, &base), 1);                 // one commit (rogue reset away)
        assert_eq!(implement::head_sha(&p).unwrap(), f.sha); // branch == the trusted (amended) tip
        assert!(p.join("A.md").exists() && p.join("B.md").exists()); // cumulative work survives
        assert!(!p.join("rogue.md").exists());           // rogue discarded
    }

    #[tokio::test]
    async fn loop_branch_switch_divergence_restores_our_branch() {
        let (_g, p, _base, sha0) = loop_repo();
        let mut fake = Fake {
            clone: p.clone(),
            verify: vec![ran_fail()],
            review: vec![rev(Verdict::Approve, 0)],
            fixes: vec![FixAct::SwitchCommit("rogue.md")],
        };
        let f = run_tweak_loop(&p, "implement/x", "task", sha0.clone(), "feat", 3, true, &mut fake).await;
        assert_eq!(f.report.stop_reason, StopReason::HeadMutated);
        assert_eq!(implement::current_branch(&p).unwrap(), "implement/x");
        assert_eq!(implement::head_sha(&p).unwrap(), sha0); // our branch back at the trusted tip
    }

    #[tokio::test]
    async fn loop_no_progress_and_fix_incomplete_and_unavailable_and_bound() {
        // no-progress: fix stages nothing.
        let (_g, p, _b, sha0) = loop_repo();
        let mut f1 = Fake { clone: p.clone(), verify: vec![ran_fail()], review: vec![rev(Verdict::Approve,0)], fixes: vec![FixAct::Nothing] };
        assert_eq!(run_tweak_loop(&p, "implement/x", "t", sha0.clone(), "feat", 3, true, &mut f1).await.report.stop_reason, StopReason::NoProgress);
        // fix-incomplete: fix returns completed=false (NOT HeadMutated).
        let (_g2, p2, _b2, s2) = loop_repo();
        let mut f2 = Fake { clone: p2.clone(), verify: vec![ran_fail()], review: vec![rev(Verdict::Approve,0)], fixes: vec![FixAct::Incomplete] };
        assert_eq!(run_tweak_loop(&p2, "implement/x", "t", s2, "feat", 3, true, &mut f2).await.report.stop_reason, StopReason::FixIncomplete);
        // fix-unavailable: actionable but no fix workflow.
        let (_g3, p3, _b3, s3) = loop_repo();
        let mut f3 = Fake { clone: p3.clone(), verify: vec![ran_fail()], review: vec![rev(Verdict::Approve,0)], fixes: vec![FixAct::Nothing] };
        assert_eq!(run_tweak_loop(&p3, "implement/x", "t", s3, "feat", 3, false, &mut f3).await.report.stop_reason, StopReason::FixUnavailable);
        // bound: max=1, persistent fail.
        let (_g4, p4, _b4, s4) = loop_repo();
        let mut f4 = Fake { clone: p4.clone(), verify: vec![ran_fail()], review: vec![rev(Verdict::Approve,0)], fixes: vec![FixAct::Stage("B.md")] };
        let r4 = run_tweak_loop(&p4, "implement/x", "t", s4, "feat", 1, true, &mut f4).await;
        assert_eq!(r4.report.stop_reason, StopReason::BoundReached);
        assert_eq!(r4.report.attempts, 1);
    }
```

Add the two verify helpers near the existing `ran`/`rev` helpers:

```rust
    fn ran_pass() -> VerifyOutcome { ran(true) }
    fn ran_fail() -> VerifyOutcome { ran(false) }
```

- [ ] **Step 2: Add the `TweakEffects` trait + `run_tweak_loop` STUBS, run — expect RED**

In `tweak.rs` (non-test), add:

```rust
#[async_trait::async_trait]
pub trait TweakEffects {
    async fn verify(&mut self, attempt: u32) -> VerifyOutcome;
    async fn review(&mut self, attempt: u32, head_sha: &str) -> (ReviewOutcome, String);
    /// Run a fix turn with `input`; returns whether the workflow COMPLETED. May mutate the clone.
    async fn fix(&mut self, attempt: u32, input: &str) -> bool;
}

pub async fn run_tweak_loop(
    _clone: &std::path::Path, _branch: &str, _task: &str, _sha: String, _original_message: &str,
    _max_attempts: u32, _fix_available: bool, _eff: &mut dyn TweakEffects,
) -> LoopFinal { todo!() }
```

Run: `cargo test -p a2a-bridge --bin a2a-bridge tweak::tests::loop_ 2>&1 | tail -20`
Expected: RED (the `todo!()` panics).

- [ ] **Step 3: Implement `run_tweak_loop`** (replace the stub)

```rust
/// The bounded review→tweak loop. Git ops run on a REAL clone; the workflow effects are injected via `eff`
/// (so the no-work-loss wiring is fake-executor testable). Phase 2: NO `?` — every fallible op → a StopReason.
pub async fn run_tweak_loop(
    clone: &std::path::Path,
    branch: &str,
    task: &str,
    mut sha: String,
    original_message: &str,
    max_attempts: u32,
    fix_available: bool,
    eff: &mut dyn TweakEffects,
) -> LoopFinal {
    use crate::implement;
    let mut attempt: u32 = 1;
    let mut last_verify = VerifyOutcome::Incomplete;
    let mut last_review = ReviewOutcome::Incomplete;
    let report = loop {
        if let Err(e) = implement::reset_worktree_to_head(clone) {
            break LoopReport { attempts: attempt, stop_reason: StopReason::StepError(e) };
        }
        last_verify = eff.verify(attempt).await;
        let (rev, synth) = eff.review(attempt, &sha).await;
        last_review = rev;
        match classify(attempt, max_attempts, &last_verify, &last_review) {
            LoopStep::Stop(reason) => break LoopReport { attempts: attempt, stop_reason: reason },
            LoopStep::Continue => {
                if !fix_available {
                    break LoopReport { attempts: attempt, stop_reason: StopReason::FixUnavailable };
                }
                let pre_i = match implement::head_sha(clone) {
                    Ok(s) => s,
                    Err(e) => break LoopReport { attempts: attempt, stop_reason: StopReason::StepError(e) },
                };
                let digest = match &last_verify {
                    VerifyOutcome::Ran(v) => crate::verify::failure_digest(v, 8 * 1024),
                    _ => String::new(),
                };
                let findings = match &last_review {
                    ReviewOutcome::Ran { verdict: Verdict::Reject, .. } => Some(synth.as_str()),
                    _ => None,
                };
                let input = build_fix_input(task, &digest, findings, 12 * 1024);
                let completed = eff.fix(attempt, &input).await;
                if !completed {
                    break LoopReport { attempts: attempt, stop_reason: StopReason::FixIncomplete };
                }
                let guard = implement::head_guard(clone, branch, &pre_i);
                let stage = match implement::stage_state(clone) {
                    Ok(s) => s,
                    Err(e) => break LoopReport { attempts: attempt, stop_reason: StopReason::StepError(e) },
                };
                // completed==true here, so `decide`'s only Abort cause is the head guard → Diverged.
                let action = implement::decide(true, guard, stage, (original_message.to_string(), false));
                match fix_step(&action) {
                    FixDisposition::Amend => match implement::host_amend_commit(clone) {
                        Ok(s) => { sha = s; attempt += 1; } // no break → loop continues
                        Err(_) => break LoopReport { attempts: attempt, stop_reason: StopReason::AmendFailed },
                    },
                    FixDisposition::Diverged => {
                        break match implement::restore_branch(clone, branch, &sha) {
                            Ok(()) => LoopReport { attempts: attempt, stop_reason: StopReason::HeadMutated },
                            Err(e) => LoopReport { attempts: attempt, stop_reason: StopReason::RestoreFailed(e) },
                        };
                    }
                    FixDisposition::NoProgress => {
                        break LoopReport { attempts: attempt, stop_reason: StopReason::NoProgress }
                    }
                }
            }
        }
    };
    LoopFinal { report, sha, last_verify, last_review }
}
```

- [ ] **Step 4: Run to verify pass** — `cargo test -p a2a-bridge --bin a2a-bridge tweak:: 2>&1 | tail -25` → PASS (all pure + loop tests).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/tweak.rs bin/a2a-bridge/Cargo.toml
git commit -m "tweak: injectable run_tweak_loop + TweakEffects seam + fake-executor tests (b2b3b)"
```

---

## Task 6: total effect helpers in main.rs (`drain_impl`, `run_verify_step`, `run_review_step`)

These are the production effect bodies `ProdEffects` (Task 7) delegates to. No new behavior; just total fns.

**Files:** Modify `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1: Add `drain_impl`** (beside `drain_review`, ~line 436)

```rust
/// Drain the implement-edit / implement-fix workflow stream → `completed`. Shared by the first edit turn and
/// every fix turn; polls to the end so the executor runs its cancel cleanup. Total (no `?`).
async fn drain_impl(mut stream: bridge_workflow::executor::WorkflowStream) -> bool {
    use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
    use futures::StreamExt;
    let mut completed = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(WorkflowEvent::NodeStarted { node }) => eprintln!("[implement] node {} started", node.as_str()),
            Ok(WorkflowEvent::NodeFinished { node, ok, .. }) =>
                eprintln!("[implement] node {} {}", node.as_str(), if ok { "ok" } else { "failed" }),
            Ok(WorkflowEvent::Terminal { outcome, .. }) => completed = matches!(outcome, WorkflowOutcome::Completed),
            Err(e) => eprintln!("[implement] error: {e:?}"),
        }
    }
    completed
}
```

- [ ] **Step 2: Add `run_verify_step` (total)**

```rust
/// Run the B2b-2 verify once (total). `verify_cfg` was captured pre-snapshot. The verdict run itself never
/// fails (a runner error becomes a failed result); a config error reduces to `ConfigError`.
fn run_verify_step(
    verify_cfg: &Option<Result<config::VerifyConfig, config::ConfigError>>,
    clone_cwd: &bridge_core::SessionCwd,
    repo: &std::path::Path,
) -> verify::VerifyOutcome {
    match verify_cfg {
        None => verify::VerifyOutcome::NotConfigured,
        Some(Err(e)) => { eprintln!("[implement] verify: config error: {e:?} — skipping verify"); verify::VerifyOutcome::ConfigError }
        Some(Ok(vcfg)) => {
            let repo_canon = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
            let cache_vol = verify::cache_volume_name(&vcfg.cache, &repo_canon.to_string_lossy());
            eprintln!("[implement] verify: running {} command(s) in {}", vcfg.commands.len(), vcfg.image);
            let verdict = verify::run_verify(vcfg, clone_cwd, &cache_vol, &verify::docker_runner, 16 * 1024);
            for r in &verdict.results {
                if !r.ok { eprintln!("[implement] verify: {} failed:\n{}", r.name, r.output); }
            }
            verify::VerifyOutcome::Ran(verdict)
        }
    }
}
```

- [ ] **Step 3: Add `run_review_step` (total, attempt-qualified, fresh token + cancel-drain)**

```rust
/// Run the B2b-3a review once (total). Returns `(outcome, synth_body)`. Fresh `CancellationToken` + `select!`
/// timeout→cancel→keep-drain PER call (so the `:ro` reaper still fires on a timed-out attempt). `run_id` is
/// qualified by `attempt`.
#[allow(clippy::too_many_arguments)]
async fn run_review_step(
    review_cfg: &Option<Result<config::ReviewConfig, config::ConfigError>>,
    wf_map: &std::collections::HashMap<bridge_core::ids::WorkflowId, std::sync::Arc<bridge_workflow::graph::WorkflowGraph>>,
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
        Some(Err(e)) => { eprintln!("[implement] review: config error: {e:?}"); return (review::ReviewOutcome::ConfigError, String::new()); }
        Some(Ok(c)) => c,
    };
    let Some(graph) = wf_map.get(&rcfg.workflow).cloned() else {
        return (review::ReviewOutcome::NotLoaded, String::new());
    };
    let input = review::build_review_input(task, base_sha, head_sha);
    let ctx = bridge_workflow::executor::WorkflowRunContext { session_cwd: Some(clone_cwd.clone()) };
    let token = tokio_util::sync::CancellationToken::new();
    let stream = executor.run_with_context(graph, input, format!("impl-review-{task_id}-{attempt}"), token.clone(), ctx);
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
        eprintln!("[implement] review {verdict:?}:\n{}", verify::truncate_output(&synth, rcfg.max_output_bytes));
    }
    (review::ReviewOutcome::Ran { verdict, summary, reviewers_failed }, synth)
}
```

- [ ] **Step 4: Build** — `cargo build -p a2a-bridge 2>&1 | tail -20` → compiles (dead-code warnings for the
three helpers are expected until Task 7 wires them).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "implement: extract total drain_impl/run_verify_step/run_review_step (b2b3b)"
```

---

## Task 7: wire `ProdEffects` + the loop into `implement_cmd`

**Files:** Modify `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1: Move `loop_cfg` resolution PRE-CLONE**

Right after `root` is canonicalized + the worktree probe (~line 470, BEFORE the task-id/clone block), add:

```rust
    // B2b-3b: resolve the loop config PRE-CLONE so a malformed [implement] fails loud before any quarantine
    // clone is created. Absent → LoopConfig::default() (loop ON, max_attempts=3).
    let loop_cfg = cfg
        .implement
        .as_ref()
        .map(|t| t.to_config())
        .transpose()
        .map_err(|e| format!("implement: [implement] config: {e}"))?
        .unwrap_or_default();
```

After `wf_map` is built (~line 531), resolve the fix graph (needs the map):

```rust
    let fix_graph = wf_map.get(&loop_cfg.fix_workflow).cloned(); // None → FixUnavailable (soft)
```

- [ ] **Step 2: Use `drain_impl` for the first edit turn**

Replace the first-edit drain (the `use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};` /
`use futures::StreamExt;` block through `drop(stream);`, ~lines 563-591) with:

```rust
    let completed = drain_impl(executor.run_with_context(
        graph, a.task.clone(), run_id, tokio_util::sync::CancellationToken::new(), ctx,
    )).await;
```

- [ ] **Step 3: Add `ProdEffects` (impl `TweakEffects`)** — place above `implement_cmd`:

```rust
/// The production `TweakEffects`: the real verify/review/fix turns. Borrows the loop's setup for its lifetime.
struct ProdEffects<'a> {
    verify_cfg: &'a Option<Result<config::VerifyConfig, config::ConfigError>>,
    review_cfg: &'a Option<Result<config::ReviewConfig, config::ConfigError>>,
    wf_map: &'a std::collections::HashMap<bridge_core::ids::WorkflowId, std::sync::Arc<bridge_workflow::graph::WorkflowGraph>>,
    executor: &'a bridge_workflow::executor::WorkflowExecutor,
    fix_graph: Option<std::sync::Arc<bridge_workflow::graph::WorkflowGraph>>,
    clone_cwd: &'a bridge_core::SessionCwd,
    repo: &'a std::path::Path,
    task: &'a str,
    base_sha: &'a str,
    task_id: &'a str,
}

#[async_trait::async_trait]
impl tweak::TweakEffects for ProdEffects<'_> {
    async fn verify(&mut self, _attempt: u32) -> verify::VerifyOutcome {
        run_verify_step(self.verify_cfg, self.clone_cwd, self.repo)
    }
    async fn review(&mut self, attempt: u32, head_sha: &str) -> (review::ReviewOutcome, String) {
        run_review_step(self.review_cfg, self.wf_map, self.executor, self.task, self.base_sha, head_sha, self.clone_cwd, self.task_id, attempt).await
    }
    async fn fix(&mut self, attempt: u32, input: &str) -> bool {
        let graph = self.fix_graph.clone().expect("fix only called when fix_available");
        drain_impl(self.executor.run_with_context(
            graph,
            input.to_string(),
            format!("impl-fix-{}-{}", self.task_id, attempt),
            tokio_util::sync::CancellationToken::new(),
            bridge_workflow::executor::WorkflowRunContext { session_cwd: Some(self.clone_cwd.clone()) },
        )).await
    }
}
```

- [ ] **Step 4: Replace the `Action::Commit` arm with the loop call + the `LoopFinal` hand-off**

Replace the entire `implement::Action::Commit(message) => { … }` arm (~lines 624-740) with:

```rust
        implement::Action::Commit(message) => {
            // Phase 1 → 2 boundary: the ONLY post-commit `?`.
            let sha = implement::host_commit(&clone, &message)?;
            let _ = std::fs::remove_file(clone.join(".git").join("A2A_COMMIT_MSG")); // R13 hygiene
            let mut effects = ProdEffects {
                verify_cfg: &verify_cfg,
                review_cfg: &review_cfg,
                wf_map: &wf_map,
                executor: &executor,
                fix_graph: fix_graph.clone(),
                clone_cwd: &clone_cwd,
                repo: &a.repo,
                task: &a.task,
                base_sha: &base_sha,
                task_id: &task_id,
            };
            let final_ = tweak::run_tweak_loop(
                &clone, &branch, &a.task, sha, &message,
                loop_cfg.max_attempts, fix_graph.is_some(), &mut effects,
            ).await;

            // Hand-off: the FINAL sha (patches the stale committed line) + the ORIGINAL subject, then the
            // verify + review + loop suffixes. Always prints.
            let subject = message.lines().next().unwrap_or("").to_string();
            let mut handoff = implement::handoff_text(
                &clone.to_string_lossy(), &branch, &final_.sha, &subject, &a.repo.to_string_lossy(),
            );
            handoff.push('\n'); handoff.push_str(&verify::outcome_suffix(&final_.last_verify));
            handoff.push('\n'); handoff.push_str(&review::outcome_suffix(&final_.last_review));
            handoff.push('\n'); handoff.push_str(&tweak::loop_outcome_suffix(&final_.report));
            println!("{handoff}");
            Ok(())
        }
```

(`message` is borrowed by `run_tweak_loop` (`&message`) and reused for `subject` after — fine, the loop
takes `&str`. `sha` moves into `run_tweak_loop` by value; the final sha comes back in `final_.sha`.)

- [ ] **Step 5: Build + bin tests + clippy**

```bash
cargo build -p a2a-bridge 2>&1 | tail -20
cargo test -p a2a-bridge --bin a2a-bridge 2>&1 | tail -25
cargo clippy -p a2a-bridge --all-targets --all-features -- -D warnings 2>&1 | tail -20
```
Expected: clean build (no dead-code/unused-import warnings), all bin tests PASS, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "implement: wire ProdEffects + run_tweak_loop into implement_cmd (b2b3b)"
```

---

## Task 8: `implement-fix` prompt + workflow + `[implement]` example (assertion red-first)

**Files:** Create `prompts/implement-fix.md`; modify `examples/a2a-bridge.containerized.toml` + the R11 test.

- [ ] **Step 1: Add the example-load assertion FIRST (red)**

In the R11 example-load test (`main.rs` ~line 2014), add (before the `implement-fix` workflow exists, so it
fails):

```rust
        assert!(
            wf.contains_key(&bridge_core::ids::WorkflowId::parse("implement-fix").unwrap()),
            "implement-fix workflow loads"
        );
```

Run: `cargo test -p a2a-bridge --bin a2a-bridge 2>&1 | grep -A2 implement-fix; cargo test -p a2a-bridge --bin a2a-bridge <that_test_name> 2>&1 | tail -10`
Expected: that test FAILS (workflow not yet defined).

- [ ] **Step 2: Write the prompt**

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

- [ ] **Step 3: Add the `[implement]` block + the workflow to the example**

After the `[review]` block (~line 22) in `examples/a2a-bridge.containerized.toml`:

```toml
# B2b-3b: the review→tweak loop. After the first commit, the bridge runs verify+review each attempt; on a
# verify-FAIL or review-REJECT it re-prompts the `impl` agent (the `implement-fix` workflow) to fix on the
# SAME clone, then AMENDs the fix into the single commit. Bounded by max_attempts (commit turns: the initial
# commit + up to max_attempts-1 fix amends). Advisory — the operator still accepts at merge. Absent → 3.
[implement]
max_attempts = 3
fix_workflow = "implement-fix"
```

After the `implement-edit` workflow block (~line 202):

```toml
# ── implement-fix (B2b-3b): one ContainerRw `impl` turn to FIX verify/review failures on the existing clone.
#    EXAMPLE-ONLY (references `impl`, which is not in the `init` scaffold). ──
[[workflows]]
id = "implement-fix"
[[workflows.nodes]]
id = "fix"
agent = "impl"
prompt_file = "../prompts/implement-fix.md"
inputs = []
```

- [ ] **Step 4: Run to verify pass** — `cargo test -p a2a-bridge --bin a2a-bridge 2>&1 | tail -15`
Expected: PASS (incl. the R11 assertion; `init_generated_config_parses_and_loads` count stays 5 —
`implement-fix` is example-only, NOT in `INIT_WORKFLOWS`).

- [ ] **Step 5: Commit**

```bash
git add prompts/implement-fix.md examples/a2a-bridge.containerized.toml bin/a2a-bridge/src/main.rs
git commit -m "implement-fix: prompt + example workflow + [implement] block (b2b3b)"
```

---

## Task 9: workspace gate + coverage

**Files:** none (verification)

> **Dead-code note:** the `pub` items in `tweak.rs` (Tasks 3+5) and the amend/reset/restore fns in
> `implement.rs` (Task 4) are unused-in-non-test-builds until Task 7 wires them; no `-D warnings` gate fires
> until Task 7/Step 5, so an executor should not misread an expected intermediate warning as an error.

- [ ] **Step 1: fmt + full workspace build/test/clippy** (serialize — do NOT parallelize heavy cargo jobs)

```bash
cargo fmt --all
cargo build --workspace 2>&1 | tail -15
cargo test --workspace --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill 2>&1 | tail -25
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -20
```
Expected: all green. (bridge-container + the two process tests need host capabilities; CI runs the full
suite — mirror the example `[verify]` exclusions.)

- [ ] **Step 2: Coverage**

```bash
cargo llvm-cov clean --workspace
cargo llvm-cov --workspace --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill 2>&1 | tail -30
```
Expected: workspace ≥ 85; bridge-core/acp/api/workflow ≥ 90. `tweak.rs` (pure + loop), `verify::failure_digest`,
`config::ImplementToml::to_config` ≥ 95. The `classify` cross-product cells are pinned by explicit tests
(Task 3) — do NOT rely on line coverage to backfill them. If under-covered, add a test; never lower a floor.

- [ ] **Step 3: Commit any fmt-only changes** (stage scoped — NOT `git add -A`)

```bash
git add $(git diff --name-only) && git commit -m "chore: cargo fmt (b2b3b)" || echo "nothing to format"
```

---

## Task 10: live gate (operator-run; Docker)

**Files:** none (manual). Prereqs: egress proxies up, `a2a-toolchain` + reader images built, creds in place.
Throwaway clone of THIS repo under `allowed_cwd_root`. Serialize heavy Docker jobs. The dangerous loop paths
(HeadMutated/RestoreFailed/FixIncomplete/NoProgress/branch-switch/no-work-loss) are covered by the Task 5
fake-executor integration tests + the Task 4 temp-repo git tests; the live gate focuses on the real-agent
happy + fix paths.

- [ ] **Step 1: Right-first-try (1 attempt)** — a trivially-correct task → hand-off shows
`verify: PASS`, `review: APPROVE`, `loop: 1 attempt(s) — converged`; ONE commit; `cherry-pick -n FETCH_HEAD`
applies it.

- [ ] **Step 2: Verify-fail → fix → pass (≥2 attempts, still one commit)** — a task with an initial
lint/test break → attempt 1 `verify: FAIL`, a `node fix started` turn, attempt 2 `verify: PASS` +
`review: APPROVE`, `loop: 2 attempt(s) — converged`, STILL one amended commit (`rev-list --count
base..HEAD == 1`).

- [ ] **Step 3: Bound reached (`max_attempts=1`)** — a failing task → one commit, `verify: FAIL`,
`loop: 1 attempt(s) — bound reached`, **exit 0**, clone left.

- [ ] **Step 4: Reaper holds** — `docker ps -a --filter name=a2a-` shows the `:rw` fix container + the `:ro`
review containers reaped (→ 0 within ~2s of each turn): confirms the fresh-token-per-iteration cancel-drain.

- [ ] **Step 5: Record** the hand-off blocks + `rev-list --count` + `docker ps -a` for the ADR.

---

## After the build

Plan dual-review is DONE (folded into this rev2). Next: inline TDD build (Tasks 1–9), live gate (Task 10),
merge + push, memory, **ADR-0023** (written post-merge per cadence — intentionally prose here, not a checkbox).

## Self-review (writing-plans)

- **Spec coverage:** silent-work-loss → `restore_branch` + `HeadMutated`/`RestoreFailed` (T4 + T5 loop +
  T5 no-work-loss integration test); config lifecycle pre-clone (T2 + T7/Step 1) + `FixUnavailable` (T5);
  total helpers + per-iteration token (T6); verify the committed tree (T4 + T5 loop top); amend keeps original
  message (T4); classify reviewers_failed + cross-product cells (T3); `build_fix_input`/`failure_digest`
  formats (T1+T3); implement-fix example-only + init count 5 (T8); attempt-qualified review id (T6); sentinels
  + final-sha hand-off patch (T5 `LoopFinal` + T7). The spec-mandated fake-executor integration test exists
  (T5). All folded.
- **Dual-review folds:** B1 matches!-move (verified false-positive; adopted `match &…`/`matches!(&…)` form in
  T5 loop) ✓; B2 no-seam → `run_tweak_loop`/`TweakEffects` + tests (T5) ✓; B3 ignored-reset + branch-switch →
  `restore_branch` matched + `RestoreFailed` (T3/T4/T5) ✓; M1 Abort-overload → `FixIncomplete` + `!completed`
  pre-check (T3/T5) ✓; M2 pre-clone config (T7/Step 1) ✓; M3 red-first (T3/T5/T8) ✓; M4 classify cells (T3) ✓;
  minors: scoped `git add` (T9), dead-code note (T9), ADR note (above) ✓.
- **Type consistency:** `VerifyOutcome::Incomplete` (T1) before use (T3/T5); `LoopConfig`/`ImplementToml`
  (T2) match T7; `TweakEffects`/`run_tweak_loop`/`LoopFinal` (T5) match the T7 call site + `ProdEffects` impl;
  `restore_branch(clone,branch,sha)` (T4) matches the T5 loop; `run_review_step` returns `(ReviewOutcome,
  String)` consumed by `ProdEffects::review`.
- **No placeholders:** every code step is complete.
