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
    NoProgress,            // a fix turn staged nothing new (NoCommitClean/Dirty)
    HeadMutated,           // a fix turn advanced/switched HEAD; the branch was restored to last-good
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
}
