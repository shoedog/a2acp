//! The `implement` review-the-diff step: after commit+verify, a 2-reviewer (codex+claude) workflow reviews
//! the committed diff and a synth node emits a `VERDICT: APPROVE|REJECT` footer. This module is the PURE
//! verdict classification + event reduction + hand-off suffix (mirrors verify.rs); the workflow RUN is
//! impure (live-gated).

/// The verdict. Inconclusive is the fail-safe — NEVER inferred Approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    Reject,
    Inconclusive,
}

/// The review step's terminal state. Every post-commit failure maps here (no `?` past the commit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewOutcome {
    Ran {
        verdict: Verdict,
        summary: String,
        reviewers_failed: usize,
    },
    NotConfigured, // no [review]
    ConfigError,   // [review] present but to_config() failed (e.g. malformed workflow id) — captured pre-commit
    NotLoaded,     // a valid workflow id absent from a successfully-loaded wf_map (typo)
    Incomplete,    // executor stream Err / missing terminal / timeout / cancel — the runtime catch-all
}

/// PURE. Tail-anchored footer parse. Exactly ONE `^VERDICT:` line must exist, and it must be the FOOTER:
/// only an immediately-following `^SUMMARY:` line (then trailing blanks) may follow. 0 or conflicting (>=2)
/// `VERDICT:` lines, an unrecognized token, or any non-footer trailing content → Inconclusive. NEVER
/// returns Approve unless an unambiguous footer `VERDICT: APPROVE` is present.
pub fn parse_verdict(synth: &str) -> (Verdict, String) {
    fn starts_ci(l: &str, kw: &str) -> bool {
        let t = l.trim_start();
        t.len() >= kw.len() && t[..kw.len()].eq_ignore_ascii_case(kw)
    }
    let lines: Vec<&str> = synth.lines().collect();
    let vidxs: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| starts_ci(l, "VERDICT:"))
        .map(|(i, _)| i)
        .collect();
    if vidxs.len() != 1 {
        return (Verdict::Inconclusive, String::new()); // none or conflicting → fail-safe
    }
    let vi = vidxs[0];
    // Tail-anchor: after the VERDICT line, allow ONLY an immediately-following SUMMARY line, then blanks.
    let mut summary = String::new();
    let mut j = vi + 1;
    if let Some(l) = lines.get(j) {
        if starts_ci(l, "SUMMARY:") {
            summary = l.trim_start()[8..].trim().to_string();
            j += 1;
        }
    }
    if lines[j..].iter().any(|l| !l.trim().is_empty()) {
        return (Verdict::Inconclusive, String::new()); // footer not at the tail → fail-safe
    }
    let token = lines[vi].trim_start()[8..].trim();
    let verdict = if token.eq_ignore_ascii_case("APPROVE") {
        Verdict::Approve
    } else if token.eq_ignore_ascii_case("REJECT") {
        Verdict::Reject
    } else {
        return (Verdict::Inconclusive, String::new());
    };
    (verdict, summary)
}

/// PURE. Reduce drained workflow events → (completed, terminal_output, reviewers_failed). Extracted from
/// the impure drain so the riskiest reduction is unit-tested (the B2b-2 keystone pattern). A failed
/// non-`synth` node = a failed reviewer leg (diversity collapse, surfaced in the suffix).
pub fn reduce(events: &[bridge_workflow::executor::WorkflowEvent]) -> (bool, String, usize) {
    use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
    let (mut completed, mut output, mut failed) = (false, String::new(), 0usize);
    for e in events {
        match e {
            WorkflowEvent::NodeFinished { node, ok, .. } if !ok && node.as_str() != "synth" => {
                failed += 1
            }
            WorkflowEvent::Terminal { outcome, output: o } => {
                completed = matches!(outcome, WorkflowOutcome::Completed);
                output = o.clone();
            }
            _ => {}
        }
    }
    (completed, output, failed)
}

/// PURE. The `{{input}}` the reviewers + synth see: the task + both host-resolved SHAs + the explicit
/// instruction to diff + navigate. The diff is NOT inlined — reviewers run `git diff` in the clone.
pub fn build_review_input(task: &str, base_sha: &str, head_sha: &str) -> String {
    format!(
        "TASK:\n{task}\n\n\
         Review the committed change in this repository: `git diff {base_sha}..{head_sha}`.\n\
         Use read-only git/grep/read to navigate the surrounding code. Assess: (1) does it DELIVER the \
         task (incl. implied requirements); (2) correctness/regressions/edge-cases; (3) design/architecture fit."
    )
}

/// PURE. The one-line hand-off suffix (mirrors verify::outcome_suffix). Encodes reviewer-leg degradation.
pub fn outcome_suffix(o: &ReviewOutcome) -> String {
    match o {
        ReviewOutcome::Ran {
            verdict,
            summary,
            reviewers_failed,
        } => {
            let label = match verdict {
                Verdict::Approve => "APPROVE",
                Verdict::Reject => "REJECT",
                Verdict::Inconclusive => "inconclusive",
            };
            let degraded = if *reviewers_failed > 0 {
                format!("  [{} reviewer(s) failed]", reviewers_failed)
            } else {
                String::new()
            };
            if summary.is_empty() {
                format!("review: {label}{degraded}")
            } else {
                format!("review: {label}  ({summary}){degraded}")
            }
        }
        ReviewOutcome::NotConfigured => "review: not configured".to_string(),
        ReviewOutcome::ConfigError => "review: skipped (config error)".to_string(),
        ReviewOutcome::NotLoaded => "review: skipped (unknown workflow)".to_string(),
        ReviewOutcome::Incomplete => "review: incomplete (did not finish)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_approve_with_summary() {
        let s = "findings...\n\nVERDICT: APPROVE\nSUMMARY: delivers the task, no blockers";
        let (v, sum) = parse_verdict(s);
        assert_eq!(v, Verdict::Approve);
        assert_eq!(sum, "delivers the task, no blockers");
    }

    #[test]
    fn parse_reject_case_insensitive_no_summary() {
        let (v, sum) = parse_verdict("blah\nverdict: reject");
        assert_eq!(v, Verdict::Reject);
        assert_eq!(sum, "");
    }

    #[test]
    fn missing_verdict_is_inconclusive() {
        assert_eq!(parse_verdict("just a review, no footer").0, Verdict::Inconclusive);
    }

    #[test]
    fn conflicting_line_start_verdicts_are_inconclusive() {
        let s = "```\nVERDICT: APPROVE\n```\n\nVERDICT: REJECT\nSUMMARY: missing X";
        assert_eq!(parse_verdict(s).0, Verdict::Inconclusive);
    }

    #[test]
    fn garbage_token_is_inconclusive_never_approve() {
        assert_eq!(parse_verdict("VERDICT: maybe").0, Verdict::Inconclusive);
    }

    #[test]
    fn finding_mentioning_approve_does_not_match() {
        let s = "MAJOR: the author wants to approve this quickly\n\nVERDICT: REJECT";
        assert_eq!(parse_verdict(s).0, Verdict::Reject);
    }

    #[test]
    fn footer_not_at_tail_is_inconclusive() {
        let s = "VERDICT: APPROVE\nMINOR: one more thing";
        assert_eq!(parse_verdict(s).0, Verdict::Inconclusive);
    }

    #[test]
    fn non_adjacent_summary_breaks_the_footer() {
        let s = "VERDICT: APPROVE\n\nSUMMARY: not adjacent";
        assert_eq!(parse_verdict(s).0, Verdict::Inconclusive);
    }

    #[test]
    fn build_input_has_task_and_both_shas_and_diff() {
        let i = build_review_input("do X", "aaa", "bbb");
        assert!(i.contains("do X") && i.contains("git diff aaa..bbb") && i.contains("DELIVER"));
    }

    #[test]
    fn outcome_suffix_all_arms() {
        let ran = ReviewOutcome::Ran {
            verdict: Verdict::Approve,
            summary: "ok".into(),
            reviewers_failed: 0,
        };
        assert_eq!(outcome_suffix(&ran), "review: APPROVE  (ok)");
        let degraded = ReviewOutcome::Ran {
            verdict: Verdict::Reject,
            summary: String::new(),
            reviewers_failed: 1,
        };
        assert_eq!(outcome_suffix(&degraded), "review: REJECT  [1 reviewer(s) failed]");
        assert_eq!(outcome_suffix(&ReviewOutcome::NotConfigured), "review: not configured");
        assert_eq!(outcome_suffix(&ReviewOutcome::ConfigError), "review: skipped (config error)");
        assert_eq!(outcome_suffix(&ReviewOutcome::NotLoaded), "review: skipped (unknown workflow)");
        assert_eq!(outcome_suffix(&ReviewOutcome::Incomplete), "review: incomplete (did not finish)");
    }

    #[test]
    fn reduce_counts_failed_reviewers_and_terminal() {
        use bridge_core::ids::NodeId;
        use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
        let ev = vec![
            WorkflowEvent::NodeFinished {
                node: NodeId::parse("reviewer_codex").unwrap(),
                ok: false,
                output: String::new(),
            },
            WorkflowEvent::NodeFinished {
                node: NodeId::parse("reviewer_claude").unwrap(),
                ok: true,
                output: "ok".into(),
            },
            WorkflowEvent::NodeFinished {
                node: NodeId::parse("synth").unwrap(),
                ok: true,
                output: "VERDICT: APPROVE".into(),
            },
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output: "VERDICT: APPROVE\nSUMMARY: ok".into(),
            },
        ];
        let (completed, output, failed) = reduce(&ev);
        assert!(completed);
        assert_eq!(failed, 1); // codex leg failed; a synth failure would NOT count as a reviewer
        assert!(output.contains("VERDICT: APPROVE"));
    }
}
