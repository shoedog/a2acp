//! The `implement` review-the-diff step: after commit+verify, a 2-reviewer (codex+claude) workflow reviews
//! the committed diff and a synth node emits a `VERDICT: APPROVE|REJECT` footer. This module is the PURE
//! verdict classification + event reduction + hand-off suffix (mirrors verify.rs); the workflow RUN is
//! impure (live-gated).

use std::path::{Path, PathBuf};

/// The verdict. Inconclusive is the fail-safe — NEVER inferred Approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    Reject,
    Inconclusive,
}

/// Adaptive-depth tier. `thorough` is deferred (see the spec) — this slice has two tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier { Light, Standard }

/// Operator depth choice. `Auto` sizes from the diff each attempt; `Forced` pins a tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth { Auto, Forced(Tier) }

impl Depth {
    /// Resolve to a concrete tier given this attempt's diff size + the config thresholds.
    pub fn resolve(self, files: usize, lines: usize, light_max_lines: usize, light_max_files: usize) -> Tier {
        match self {
            Depth::Forced(t) => t,
            Depth::Auto => select_tier(files, lines, light_max_lines, light_max_files),
        }
    }
}

/// PURE. light iff `lines <= light_max_lines` AND `files <= light_max_files`; else standard.
pub fn select_tier(files: usize, lines: usize, light_max_lines: usize, light_max_files: usize) -> Tier {
    if lines <= light_max_lines && files <= light_max_files { Tier::Light } else { Tier::Standard }
}

/// PURE. Parse `git diff --numstat` → (changed_files, added+deleted_lines). A binary row (`-\t-\t<path>`)
/// counts toward files only. Malformed lines are skipped (the caller treats a total parse failure as standard).
pub fn parse_numstat(stdout: &str) -> (usize, usize) {
    let (mut files, mut lines) = (0usize, 0usize);
    for row in stdout.lines() {
        let mut cols = row.splitn(3, '\t');
        let (Some(a), Some(d), Some(_path)) = (cols.next(), cols.next(), cols.next()) else { continue };
        files += 1;
        if let (Ok(added), Ok(deleted)) = (a.parse::<usize>(), d.parse::<usize>()) {
            lines += added + deleted;
        }
    }
    (files, lines)
}

/// PURE. The slice reference-file path, UNDER `.git/` so it survives `git reset --hard && git clean -fdq`,
/// never dirties the worktree, never blocks `--resume`, and is invisible to the write-capable fix turn.
pub fn slice_ref_path(clone: &Path, runid: &str) -> PathBuf {
    clone.join(".git/a2a-bridge/review-slices").join(format!("slice-{runid}.md"))
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
    ConfigError, // [review] present but to_config() failed (e.g. malformed workflow id) — captured pre-commit
    NotLoaded,   // a valid workflow id absent from a successfully-loaded wf_map (typo)
    Incomplete, // executor stream Err / missing terminal / timeout / cancel — the runtime catch-all
}

/// PURE. Tail-anchored footer parse. The LAST `^VERDICT:` line is the footer and must be anchored: only an
/// immediately-following `^SUMMARY:` line (then trailing blanks) may follow it. A synth model often restates
/// its verdict (a lead `VERDICT: X` AND the mandated footer `VERDICT: X`) — that is fine when every VERDICT
/// line AGREES; a genuine CONFLICT (e.g. a body `APPROVE` + footer `REJECT`), an unrecognized token, no
/// VERDICT line, or non-footer trailing content → Inconclusive. NEVER returns Approve unless an unambiguous,
/// unconflicted footer `VERDICT: APPROVE` is present. (Live gate: the synth led with + footed APPROVE → the
/// old "exactly one VERDICT line" rule wrongly read agreement as Inconclusive.)
pub fn parse_verdict(synth: &str) -> (Verdict, String) {
    fn starts_ci(l: &str, kw: &str) -> bool {
        // Compare BYTES (the keywords are pure ASCII) — slicing `&str[..kw.len()]` panics when a
        // multi-byte char (e.g. an em-dash in a finding like "MAJOR — none.") straddles the boundary.
        let b = l.trim_start().as_bytes();
        b.len() >= kw.len() && b[..kw.len()].eq_ignore_ascii_case(kw.as_bytes())
    }
    // The token after "VERDICT:" (8 ASCII bytes; the line is in `vidxs` so trim_start().len() >= 8).
    fn verdict_token(line: &str) -> Option<Verdict> {
        let t = line.trim_start()[8..].trim();
        if t.eq_ignore_ascii_case("APPROVE") {
            Some(Verdict::Approve)
        } else if t.eq_ignore_ascii_case("REJECT") {
            Some(Verdict::Reject)
        } else {
            None
        }
    }
    let lines: Vec<&str> = synth.lines().collect();
    let vidxs: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| starts_ci(l, "VERDICT:"))
        .map(|(i, _)| i)
        .collect();
    let Some(&vi) = vidxs.last() else {
        return (Verdict::Inconclusive, String::new()); // no VERDICT line → fail-safe
    };
    // Tail-anchor on the LAST VERDICT line: after it, allow ONLY an immediately-following SUMMARY, then blanks.
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
    // The footer token, and every VERDICT line must agree with it (a restatement is fine; a conflict is not).
    let Some(footer) = verdict_token(lines[vi]) else {
        return (Verdict::Inconclusive, String::new());
    };
    if vidxs
        .iter()
        .any(|&i| verdict_token(lines[i]).as_ref() != Some(&footer))
    {
        return (Verdict::Inconclusive, String::new()); // conflicting / unrecognized earlier verdict
    }
    (footer, summary)
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
/// Pass `slice_ref` to point reviewers at a prism review-slice file under `.git/a2a-bridge/review-slices/`.
pub fn build_review_input(task: &str, base_sha: &str, head_sha: &str, slice_ref: Option<&str>) -> String {
    let slice = match slice_ref {
        Some(path) => format!(
            "\nA prism review-slice (defect-focused: blast radius, taint paths, missing symmetry) for this \
             diff is at `{path}` — read it FIRST as a map of where to look, then verify against the code.\n"
        ),
        None => String::new(),
    };
    format!(
        "TASK:\n{task}\n\n\
         Review the committed change in this repository: `git diff {base_sha}..{head_sha}`.\n\
         Use read-only git/grep/read to navigate the surrounding code. Assess: (1) does it DELIVER the \
         task (incl. implied requirements); (2) correctness/regressions/edge-cases; (3) design/architecture fit.{slice}"
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
        assert_eq!(
            parse_verdict("just a review, no footer").0,
            Verdict::Inconclusive
        );
    }

    #[test]
    fn conflicting_line_start_verdicts_are_inconclusive() {
        let s = "```\nVERDICT: APPROVE\n```\n\nVERDICT: REJECT\nSUMMARY: missing X";
        assert_eq!(parse_verdict(s).0, Verdict::Inconclusive);
    }

    #[test]
    fn restated_agreeing_verdict_takes_the_footer() {
        // Live-gate shape: the synth led with VERDICT: APPROVE and footed VERDICT: APPROVE — agreement, not
        // a conflict, so the footer wins (the old "exactly one VERDICT line" rule wrongly said Inconclusive).
        let s = "VERDICT: APPROVE\n\nmerged review, no findings.\n\nVERDICT: APPROVE\nSUMMARY: both agree";
        assert_eq!(
            parse_verdict(s),
            (Verdict::Approve, "both agree".to_string())
        );
        // a restated REJECT likewise resolves to REJECT.
        let r = "VERDICT: REJECT\n\nblocker found.\n\nVERDICT: REJECT\nSUMMARY: blocker";
        assert_eq!(parse_verdict(r).0, Verdict::Reject);
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
    fn multibyte_finding_line_does_not_panic() {
        // an em-dash (3 bytes) before byte 8 used to panic the byte-slice prefix check
        let s = "MAJOR — none.\nMINOR — tidy up.\n\nVERDICT: APPROVE\nSUMMARY: ok";
        assert_eq!(parse_verdict(s), (Verdict::Approve, "ok".to_string()));
    }

    #[test]
    fn build_input_no_slice_has_task_shas_diff() {
        let i = build_review_input("do X", "aaa", "bbb", None);
        assert!(i.contains("do X") && i.contains("git diff aaa..bbb") && i.contains("DELIVER"));
        assert!(!i.contains("prism review-slice"));
    }
    #[test]
    fn build_input_with_slice_points_at_the_ref_file() {
        let i = build_review_input("do X", "aaa", "bbb", Some(".git/a2a-bridge/review-slices/slice-1.md"));
        assert!(i.contains("prism review-slice") && i.contains("slice-1.md"));
    }

    #[test]
    fn slice_ref_path_lives_under_git_a2a_bridge() {
        let p = slice_ref_path(std::path::Path::new("/clone"), "task-7-1");
        assert_eq!(p, std::path::PathBuf::from("/clone/.git/a2a-bridge/review-slices/slice-task-7-1.md"));
    }

    #[test]
    fn parse_numstat_sums_added_deleted_and_counts_files() {
        let out = "3\t1\tsrc/a.rs\n10\t0\tsrc/b.rs\n-\t-\tlogo.png\n";
        assert_eq!(parse_numstat(out), (3, 14));
    }
    #[test]
    fn parse_numstat_empty_is_zero() { assert_eq!(parse_numstat(""), (0, 0)); }

    #[test]
    fn select_tier_light_requires_both_under_thresholds() {
        assert_eq!(select_tier(2, 10, 15, 2), Tier::Light);
        assert_eq!(select_tier(2, 15, 15, 2), Tier::Light);
        assert_eq!(select_tier(3, 10, 15, 2), Tier::Standard);
        assert_eq!(select_tier(1, 16, 15, 2), Tier::Standard);
    }
    #[test]
    fn resolve_depth_forced_overrides_auto() {
        assert_eq!(Depth::Forced(Tier::Standard).resolve(0, 0, 15, 2), Tier::Standard);
        assert_eq!(Depth::Auto.resolve(0, 0, 15, 2), Tier::Light);
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
        assert_eq!(
            outcome_suffix(&degraded),
            "review: REJECT  [1 reviewer(s) failed]"
        );
        assert_eq!(
            outcome_suffix(&ReviewOutcome::NotConfigured),
            "review: not configured"
        );
        assert_eq!(
            outcome_suffix(&ReviewOutcome::ConfigError),
            "review: skipped (config error)"
        );
        assert_eq!(
            outcome_suffix(&ReviewOutcome::NotLoaded),
            "review: skipped (unknown workflow)"
        );
        assert_eq!(
            outcome_suffix(&ReviewOutcome::Incomplete),
            "review: incomplete (did not finish)"
        );
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
