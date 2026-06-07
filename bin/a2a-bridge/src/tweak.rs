//! The B2b-3b review→tweak loop. PURE core (`classify`/`fix_step`/`build_fix_input`/`loop_outcome_suffix`)
//! plus the injectable `run_tweak_loop` driven through the `TweakEffects` seam — so the no-work-loss wiring
//! is unit-tested with a FAKE executor while the git ops run on a REAL clone. No panics, no slicing
//! (B2b-3a's em-dash lesson); phase-2 totality (no `?`).

use crate::review::{ReviewOutcome, Verdict};
use crate::verify::VerifyOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Success,
    BoundReached,
    NotActionable,
    NoProgress,            // a fix turn staged nothing new (NoCommitClean/Dirty)
    HeadMutated, // a fix turn advanced/switched HEAD; the branch was restored to last-good
    RestoreFailed(String), // HEAD diverged AND restoring the branch failed → the branch tip is UNTRUSTED
    FixIncomplete,         // the fix workflow did not complete (NOT a HEAD mutation)
    AmendFailed,
    StepError(String), // a post-commit git op (reset/stage/head) failed — reduced, never `?`
    FixUnavailable,    // actionable but no fix workflow is registered
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixDisposition {
    Amend,
    Diverged,
    NoProgress,
}

/// The final loop state for the hand-off (the report + the FINAL sha + the LAST verify/review outcomes).
#[derive(Debug)]
pub struct LoopFinal {
    pub report: LoopReport,
    pub sha: String,
    pub last_verify: VerifyOutcome,
    pub last_review: ReviewOutcome,
}

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
    let review_actionable = matches!(
        r,
        ReviewOutcome::Ran {
            verdict: Verdict::Reject,
            ..
        }
    );
    if !(verify_actionable || review_actionable) {
        return LoopStep::Stop(StopReason::NotActionable);
    }
    if attempt >= max_attempts {
        return LoopStep::Stop(StopReason::BoundReached);
    }
    LoopStep::Continue
}

pub fn build_fix_input(
    task: &str,
    verify_digest: &str,
    review_findings: Option<&str>,
    max_bytes: usize,
) -> String {
    let header = format!(
        "{task}\n\nThe previous attempt did not pass. FIX the issues below; re-stage your fixes with \
         `git add` (the bridge folds ONLY staged changes); do NOT run `git commit` and do NOT write a commit \
         message.\n"
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
        StopReason::RestoreFailed(e) => format!(
            "fix turn diverged HEAD and the branch is UNTRUSTED (restore failed: {e}) — inspect the \
             clone; do NOT use the merge command above"
        ),
        StopReason::FixIncomplete => "fix turn did not complete".to_string(),
        StopReason::AmendFailed => "amend failed".to_string(),
        StopReason::FixUnavailable => "no fix workflow configured".to_string(),
        StopReason::StepError(e) => format!("step error: {e}"),
    };
    format!("loop: {} attempt(s) — {}", rep.attempts, why)
}

/// The injected workflow effects (verify/review/fix). Production wires `ProdEffects` (executor-backed); tests
/// wire a scripted fake — so the loop's no-work-loss wiring is unit-tested without a real executor.
#[async_trait::async_trait]
pub trait TweakEffects {
    async fn verify(&mut self, attempt: u32) -> VerifyOutcome;
    async fn review(&mut self, attempt: u32, head_sha: &str) -> (ReviewOutcome, String);
    /// Run a fix turn with `input`; returns whether the workflow COMPLETED. May mutate the clone.
    async fn fix(&mut self, attempt: u32, input: &str) -> bool;
}

/// The bounded review→tweak loop. Git ops run on a REAL clone; the workflow effects are injected via `eff`
/// (so the no-work-loss wiring is fake-executor testable). Phase 2: NO `?` — every fallible op → a StopReason.
#[allow(clippy::too_many_arguments)]
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
        // Verify the COMMITTED tree (discard the agent's unstaged scratch first).
        if let Err(e) = implement::reset_worktree_to_head(clone) {
            break LoopReport {
                attempts: attempt,
                stop_reason: StopReason::StepError(e),
            };
        }
        last_verify = eff.verify(attempt).await;
        let (rev, synth) = eff.review(attempt, &sha).await;
        last_review = rev;
        match classify(attempt, max_attempts, &last_verify, &last_review) {
            LoopStep::Stop(reason) => {
                break LoopReport {
                    attempts: attempt,
                    stop_reason: reason,
                }
            }
            LoopStep::Continue => {
                if !fix_available {
                    break LoopReport {
                        attempts: attempt,
                        stop_reason: StopReason::FixUnavailable,
                    };
                }
                let pre_i = match implement::head_sha(clone) {
                    Ok(s) => s,
                    Err(e) => {
                        break LoopReport {
                            attempts: attempt,
                            stop_reason: StopReason::StepError(e),
                        }
                    }
                };
                let digest = match &last_verify {
                    VerifyOutcome::Ran(v) => crate::verify::failure_digest(v, 8 * 1024),
                    _ => String::new(),
                };
                let findings = match &last_review {
                    ReviewOutcome::Ran {
                        verdict: Verdict::Reject,
                        ..
                    } => Some(synth.as_str()),
                    _ => None,
                };
                let input = build_fix_input(task, &digest, findings, 12 * 1024);
                let completed = eff.fix(attempt, &input).await;
                if !completed {
                    break LoopReport {
                        attempts: attempt,
                        stop_reason: StopReason::FixIncomplete,
                    };
                }
                let guard = implement::head_guard(clone, branch, &pre_i);
                let stage = match implement::stage_state(clone) {
                    Ok(s) => s,
                    Err(e) => {
                        break LoopReport {
                            attempts: attempt,
                            stop_reason: StopReason::StepError(e),
                        }
                    }
                };
                // completed==true here, so `decide`'s only Abort cause is the head guard → Diverged.
                let action =
                    implement::decide(true, guard, stage, (original_message.to_string(), false));
                match fix_step(&action) {
                    FixDisposition::Amend => match implement::host_amend_commit(clone) {
                        Ok(s) => {
                            sha = s;
                            attempt += 1;
                        } // no break → loop continues
                        Err(_) => {
                            break LoopReport {
                                attempts: attempt,
                                stop_reason: StopReason::AmendFailed,
                            }
                        }
                    },
                    FixDisposition::Diverged => {
                        break match implement::restore_branch(clone, branch, &sha) {
                            Ok(()) => LoopReport {
                                attempts: attempt,
                                stop_reason: StopReason::HeadMutated,
                            },
                            Err(e) => LoopReport {
                                attempts: attempt,
                                stop_reason: StopReason::RestoreFailed(e),
                            },
                        };
                    }
                    FixDisposition::NoProgress => {
                        break LoopReport {
                            attempts: attempt,
                            stop_reason: StopReason::NoProgress,
                        }
                    }
                }
            }
        }
    };
    LoopFinal {
        report,
        sha,
        last_verify,
        last_review,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::{aggregate, VerifyResult};

    fn ran(passed: bool) -> VerifyOutcome {
        VerifyOutcome::Ran(aggregate(vec![VerifyResult {
            name: "t".into(),
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
    fn ran_pass() -> VerifyOutcome {
        ran(true)
    }
    fn ran_fail() -> VerifyOutcome {
        ran(false)
    }

    #[test]
    fn success_when_both_ok_incl_degraded_approve() {
        assert_eq!(
            classify(1, 3, &ran(true), &rev(Verdict::Approve, 0)),
            LoopStep::Stop(StopReason::Success)
        );
        assert_eq!(
            classify(1, 3, &ran(true), &rev(Verdict::Approve, 1)),
            LoopStep::Stop(StopReason::Success)
        );
        assert_eq!(
            classify(
                1,
                1,
                &VerifyOutcome::NotConfigured,
                &ReviewOutcome::NotConfigured
            ),
            LoopStep::Stop(StopReason::Success)
        );
    }

    #[test]
    fn continue_when_actionable_under_bound() {
        assert_eq!(
            classify(1, 3, &ran(false), &rev(Verdict::Approve, 0)),
            LoopStep::Continue
        );
        assert_eq!(
            classify(1, 3, &ran(true), &rev(Verdict::Reject, 0)),
            LoopStep::Continue
        );
        assert_eq!(
            classify(1, 3, &ran(true), &rev(Verdict::Reject, 1)),
            LoopStep::Continue
        );
        assert_eq!(
            classify(1, 3, &ran(false), &ReviewOutcome::NotConfigured),
            LoopStep::Continue
        );
        // cross-product cell: verify ConfigError but review Reject → still actionable (OR), NOT vetoed.
        assert_eq!(
            classify(1, 3, &VerifyOutcome::ConfigError, &rev(Verdict::Reject, 0)),
            LoopStep::Continue
        );
    }

    #[test]
    fn bound_reached_at_max() {
        assert_eq!(
            classify(3, 3, &ran(false), &rev(Verdict::Reject, 0)),
            LoopStep::Stop(StopReason::BoundReached)
        );
        assert_eq!(
            classify(1, 1, &ran(false), &ReviewOutcome::NotConfigured),
            LoopStep::Stop(StopReason::BoundReached)
        );
    }

    #[test]
    fn not_actionable_cross_product() {
        assert_eq!(
            classify(1, 3, &ran(true), &rev(Verdict::Inconclusive, 0)),
            LoopStep::Stop(StopReason::NotActionable)
        );
        assert_eq!(
            classify(1, 3, &VerifyOutcome::Incomplete, &rev(Verdict::Approve, 1)),
            LoopStep::Stop(StopReason::NotActionable)
        );
        assert_eq!(
            classify(1, 3, &VerifyOutcome::ConfigError, &rev(Verdict::Approve, 0)),
            LoopStep::Stop(StopReason::NotActionable)
        );
        assert_eq!(
            classify(
                1,
                3,
                &VerifyOutcome::ConfigError,
                &ReviewOutcome::Incomplete
            ),
            LoopStep::Stop(StopReason::NotActionable)
        );
        assert_eq!(
            classify(
                1,
                3,
                &VerifyOutcome::NotConfigured,
                &ReviewOutcome::NotLoaded
            ),
            LoopStep::Stop(StopReason::NotActionable)
        );
    }

    #[test]
    fn fix_step_maps_each_action() {
        use crate::implement::Action;
        assert_eq!(fix_step(&Action::Commit("m".into())), FixDisposition::Amend);
        assert_eq!(
            fix_step(&Action::Abort("x".into())),
            FixDisposition::Diverged
        );
        assert_eq!(fix_step(&Action::NoCommitClean), FixDisposition::NoProgress);
        assert_eq!(fix_step(&Action::NoCommitDirty), FixDisposition::NoProgress);
    }

    #[test]
    fn build_fix_input_keeps_task_and_sections() {
        let i = build_fix_input("do X", "### clippy\nerr", Some("BLOCKER: bug"), 4096);
        assert!(i.contains("do X") && i.contains("## Verify failures") && i.contains("### clippy"));
        assert!(i.contains("## Review findings (REJECTED)") && i.contains("BLOCKER: bug"));
        // Warm-session framing: self-sufficient (task + git-add mandate), no "prior commit" assumption.
        assert!(i.contains("git add"));
        assert!(!i.contains("prior commit"));
        let v = build_fix_input("do X", "### test\nfail", None, 4096);
        assert!(v.contains("## Verify failures") && !v.contains("Review findings"));
        let r = build_fix_input("do X", "", Some("MAJOR: y"), 4096);
        assert!(!r.contains("## Verify failures") && r.contains("Review findings"));
        let t = build_fix_input("do X", &"E".repeat(9000), Some(&"R".repeat(9000)), 256);
        assert!(t.contains("do X"));
    }

    #[test]
    fn loop_outcome_suffix_all_reasons() {
        let mk = |r: StopReason| {
            loop_outcome_suffix(&LoopReport {
                attempts: 2,
                stop_reason: r,
            })
        };
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

    // ── run_tweak_loop: REAL clone + FAKE executor (the no-work-loss seam) ──────────────────────────

    use crate::implement;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn git(p: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(p)
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git {args:?}"
        );
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
    enum FixAct {
        Stage(&'static str),
        Nothing,
        SelfCommit(&'static str),
        SwitchCommit(&'static str),
        Incomplete,
    }

    struct Fake {
        clone: PathBuf,
        verify: Vec<VerifyOutcome>,
        review: Vec<ReviewOutcome>,
        fixes: Vec<FixAct>,
    }
    fn at<T: Clone>(v: &[T], i: u32) -> T {
        v[((i as usize).saturating_sub(1)).min(v.len() - 1)].clone()
    }

    #[async_trait::async_trait]
    impl TweakEffects for Fake {
        async fn verify(&mut self, attempt: u32) -> VerifyOutcome {
            at(&self.verify, attempt)
        }
        async fn review(&mut self, attempt: u32, _head: &str) -> (ReviewOutcome, String) {
            (at(&self.review, attempt), "BLOCKER: synth body".into())
        }
        async fn fix(&mut self, attempt: u32, _input: &str) -> bool {
            match at(&self.fixes, attempt) {
                FixAct::Stage(f) => {
                    std::fs::write(self.clone.join(f), "x\n").unwrap();
                    git(&self.clone, &["add", f]);
                    true
                }
                FixAct::Nothing => true,
                FixAct::SelfCommit(f) => {
                    std::fs::write(self.clone.join(f), "x\n").unwrap();
                    git(&self.clone, &["add", f]);
                    git(&self.clone, &["commit", "-q", "-m", "rogue"]);
                    true
                }
                FixAct::SwitchCommit(f) => {
                    git(&self.clone, &["checkout", "-q", "-b", "rogue-b"]);
                    std::fs::write(self.clone.join(f), "x\n").unwrap();
                    git(&self.clone, &["add", f]);
                    git(&self.clone, &["commit", "-q", "-m", "rogue"]);
                    true
                }
                FixAct::Incomplete => false,
            }
        }
    }

    fn ahead(p: &Path, base: &str) -> usize {
        let o = implement::run_git(Some(p), &["rev-list", "--count", &format!("{base}..HEAD")])
            .unwrap();
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
        assert_eq!(ahead(&p, &base), 1); // one commit (rogue reset away)
        assert_eq!(implement::head_sha(&p).unwrap(), f.sha); // branch == the trusted (amended) tip
        assert!(p.join("A.md").exists() && p.join("B.md").exists()); // cumulative work survives
        assert!(!p.join("rogue.md").exists()); // rogue discarded
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
        let f = run_tweak_loop(
            &p,
            "implement/x",
            "task",
            sha0.clone(),
            "feat",
            3,
            true,
            &mut fake,
        )
        .await;
        assert_eq!(f.report.stop_reason, StopReason::HeadMutated);
        assert_eq!(implement::current_branch(&p).unwrap(), "implement/x");
        assert_eq!(implement::head_sha(&p).unwrap(), sha0); // our branch back at the trusted tip
    }

    #[tokio::test]
    async fn loop_no_progress_and_fix_incomplete_and_unavailable_and_bound() {
        // no-progress: fix stages nothing.
        let (_g, p, _b, sha0) = loop_repo();
        let mut f1 = Fake {
            clone: p.clone(),
            verify: vec![ran_fail()],
            review: vec![rev(Verdict::Approve, 0)],
            fixes: vec![FixAct::Nothing],
        };
        assert_eq!(
            run_tweak_loop(&p, "implement/x", "t", sha0, "feat", 3, true, &mut f1)
                .await
                .report
                .stop_reason,
            StopReason::NoProgress
        );
        // fix-incomplete: fix returns completed=false (NOT HeadMutated).
        let (_g2, p2, _b2, s2) = loop_repo();
        let mut f2 = Fake {
            clone: p2.clone(),
            verify: vec![ran_fail()],
            review: vec![rev(Verdict::Approve, 0)],
            fixes: vec![FixAct::Incomplete],
        };
        assert_eq!(
            run_tweak_loop(&p2, "implement/x", "t", s2, "feat", 3, true, &mut f2)
                .await
                .report
                .stop_reason,
            StopReason::FixIncomplete
        );
        // fix-unavailable: actionable but no fix workflow.
        let (_g3, p3, _b3, s3) = loop_repo();
        let mut f3 = Fake {
            clone: p3.clone(),
            verify: vec![ran_fail()],
            review: vec![rev(Verdict::Approve, 0)],
            fixes: vec![FixAct::Nothing],
        };
        assert_eq!(
            run_tweak_loop(&p3, "implement/x", "t", s3, "feat", 3, false, &mut f3)
                .await
                .report
                .stop_reason,
            StopReason::FixUnavailable
        );
        // bound: max=1, persistent fail.
        let (_g4, p4, _b4, s4) = loop_repo();
        let mut f4 = Fake {
            clone: p4.clone(),
            verify: vec![ran_fail()],
            review: vec![rev(Verdict::Approve, 0)],
            fixes: vec![FixAct::Stage("B.md")],
        };
        let r4 = run_tweak_loop(&p4, "implement/x", "t", s4, "feat", 1, true, &mut f4).await;
        assert_eq!(r4.report.stop_reason, StopReason::BoundReached);
        assert_eq!(r4.report.attempts, 1);
    }
}
