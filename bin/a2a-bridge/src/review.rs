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

/// Adaptive-depth tier. `thorough` = draft→refine double pass for large code/infra diffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Light,
    Standard,
    Thorough,
}

/// Operator depth choice. `Auto` sizes from the diff each attempt; `Forced` pins a tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Auto,
    Forced(Tier),
}

impl Depth {
    /// Parse a `--depth` / `default_depth` value. "auto" -> Auto, else Forced(tier); unknown -> Err.
    pub fn parse_flag(s: &str) -> Result<Depth, String> {
        match s {
            "auto" => Ok(Depth::Auto),
            "light" => Ok(Depth::Forced(Tier::Light)),
            "standard" => Ok(Depth::Forced(Tier::Standard)),
            "thorough" => Ok(Depth::Forced(Tier::Thorough)),
            other => Err(format!(
                "--depth: unknown value {other:?} (expected auto|light|standard|thorough)"
            )),
        }
    }

    /// Reconstruct from a checkpoint `forced_depth` string; `None`/unknown -> Auto.
    pub fn from_forced_str(s: Option<&str>) -> Depth {
        match s {
            Some("light") => Depth::Forced(Tier::Light),
            Some("standard") => Depth::Forced(Tier::Standard),
            Some("thorough") => Depth::Forced(Tier::Thorough),
            _ => Depth::Auto,
        }
    }

    /// Serialize to a checkpoint `forced_depth` string; Auto -> None.
    pub fn to_forced_str(self) -> Option<String> {
        match self {
            Depth::Forced(Tier::Light) => Some("light".into()),
            Depth::Forced(Tier::Standard) => Some("standard".into()),
            Depth::Forced(Tier::Thorough) => Some("thorough".into()),
            Depth::Auto => None,
        }
    }

    /// Resolve to a concrete tier. `Forced` always wins; `Auto` sizes from `sizing` when known, else
    /// (git/parse failure) falls back to `Standard` — NEVER `Thorough` (the unknown-size fail-safe).
    /// `sizing` is `(files, lines)` over code/infra only.
    pub fn resolve(
        self,
        sizing: Option<(usize, usize)>,
        light_max_lines: usize,
        light_max_files: usize,
        thorough_min_lines: usize,
        thorough_min_files: usize,
    ) -> Tier {
        match (self, sizing) {
            (Depth::Forced(t), _) => t,
            (Depth::Auto, Some((files, lines))) => select_tier(
                files,
                lines,
                light_max_lines,
                light_max_files,
                thorough_min_lines,
                thorough_min_files,
            ),
            (Depth::Auto, None) => Tier::Standard,
        }
    }
}

/// PURE. light iff `files ≤ light_max_files` AND `lines ≤ light_max_lines`; else thorough iff
/// `lines ≥ thorough_min_lines` OR `files ≥ thorough_min_files`; else standard. Light is checked first;
/// config validation guarantees `thorough_min_* > light_max_*` so the bands cannot overlap.
pub fn select_tier(
    files: usize,
    lines: usize,
    light_max_lines: usize,
    light_max_files: usize,
    thorough_min_lines: usize,
    thorough_min_files: usize,
) -> Tier {
    if lines <= light_max_lines && files <= light_max_files {
        Tier::Light
    } else if lines >= thorough_min_lines || files >= thorough_min_files {
        Tier::Thorough
    } else {
        Tier::Standard
    }
}

/// PURE. The workflow-id suffix for a tier (`Standard` = the base workflow, no suffix).
pub fn tier_workflow_suffix(tier: Tier) -> &'static str {
    match tier {
        Tier::Light => "-light",
        Tier::Standard => "",
        Tier::Thorough => "-thorough",
    }
}

/// PURE. Does this path count toward review-depth sizing? Markdown, tests, lockfiles, and generated files
/// are excluded so depth reflects the CODE+INFRA change. Ambiguous paths COUNT (bias toward review).
pub fn counts_toward_depth(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        return false;
    }
    const LOCKFILES: &[&str] = &[
        "cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "gemfile.lock",
        "poetry.lock",
        "pipfile.lock",
        "composer.lock",
        "go.sum",
    ];
    if LOCKFILES.contains(&file) || lower.ends_with(".lock") {
        return false;
    }
    if lower.ends_with(".min.js")
        || lower.ends_with(".min.css")
        || lower.ends_with(".pb.go")
        || lower.ends_with(".snap")
        || lower.ends_with(".g.dart")
        || lower.contains("_pb2.")
        || lower.contains(".generated.")
        || lower.contains("_generated.")
    {
        return false;
    }
    const TEST_DIRS: &[&str] = &[
        "tests",
        "test",
        "__tests__",
        "__mocks__",
        "testdata",
        "fixtures",
    ];
    const GEN_DIRS: &[&str] = &["node_modules", "vendor", "dist", "target", ".next"];
    if lower
        .split('/')
        .any(|c| TEST_DIRS.contains(&c) || GEN_DIRS.contains(&c))
    {
        return false;
    }
    if file.contains("_test.")
        || file.contains("_tests.")
        || file.starts_with("test_")
        || file.contains(".test.")
        || file.contains(".spec.")
        || file.contains("_spec.")
    {
        return false;
    }
    true
}

/// PURE. Comment markers for a path's language (line-comment + block-open). Empty = strip nothing.
fn comment_markers(path: &str) -> &'static [&'static str] {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    if file == "dockerfile" || file == "containerfile" || file.ends_with(".dockerfile") {
        return &["#"];
    }
    let ext = file.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => &["//"], // NOT `#` — `#[derive]`/`#![…]` attributes are CODE
        "toml" | "yaml" | "yml" | "sh" | "bash" | "py" | "rb" | "cfg" | "ini" | "conf" => &["#"],
        "c" | "h" | "cpp" | "hpp" | "cc" | "go" | "js" | "jsx" | "ts" | "tsx" | "java" | "kt"
        | "swift" => &["//", "/*"],
        "sql" | "lua" | "hs" => &["--"],
        "html" | "xml" | "vue" | "svelte" => &["<!--"],
        _ => &[],
    }
}

/// PURE, path-aware. A changed line is "logical" unless blank or a leading comment for that language.
/// Approximate LLOC (block-comment interiors and string-embedded markers are known, conservative limits).
pub fn is_logical_line(path: &str, content: &str) -> bool {
    let t = content.trim_start();
    if t.is_empty() {
        return false;
    }
    !comment_markers(path).iter().any(|m| t.starts_with(m))
}

/// PURE. Best-effort unquote of a Git C-quoted path (`"a/x\ty"`). With `core.quotePath=false` the only
/// remaining escapes are control chars / quotes; anything undecoded falls through (fail-open → counts).
fn unquote_path(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('t') => out.push('\t'),
                    Some('n') => out.push('\n'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => out.push(other),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        s.to_string()
    }
}

/// PURE. The path from a `--- <s>` / `+++ <s>` header: unquote, drop the `a/`/`b/` prefix, `/dev/null`→None.
fn header_path(s: &str, side: char) -> Option<String> {
    let s = s.trim();
    if s == "/dev/null" {
        return None;
    }
    let unq = unquote_path(s);
    let prefix = format!("{side}/");
    Some(unq.strip_prefix(&prefix).unwrap_or(&unq).to_string())
}

/// PURE, HUNK-STATEFUL + side-aware. Parse a unified-diff patch → (code_infra_files, lloc). File paths
/// come from the header phase; lines are classified ONLY inside hunk phase by their leading byte. A `+`
/// line is gated by the NEW path, a `-` line by the OLD path; a file counts iff it has ≥1 logical line.
pub fn parse_diff_for_depth(patch: &str) -> (usize, usize) {
    let mut files = 0usize;
    let mut lloc = 0usize;
    let mut old_path: Option<String> = None;
    let mut new_path: Option<String> = None;
    let mut file_logical = 0usize;
    let mut in_hunk = false;

    for raw in patch.lines() {
        if raw.starts_with("diff --git ") {
            if file_logical > 0 {
                files += 1;
                lloc += file_logical;
            }
            old_path = None;
            new_path = None;
            file_logical = 0;
            in_hunk = false;
            continue;
        }
        if !in_hunk {
            if let Some(p) = raw.strip_prefix("--- ") {
                old_path = header_path(p, 'a');
            } else if let Some(p) = raw.strip_prefix("+++ ") {
                new_path = header_path(p, 'b');
            } else if let Some(p) = raw.strip_prefix("rename from ") {
                old_path = Some(unquote_path(p));
            } else if let Some(p) = raw.strip_prefix("rename to ") {
                new_path = Some(unquote_path(p));
            } else if let Some(p) = raw.strip_prefix("copy from ") {
                old_path = Some(unquote_path(p));
            } else if let Some(p) = raw.strip_prefix("copy to ") {
                new_path = Some(unquote_path(p));
            } else if raw.starts_with("@@") {
                in_hunk = true;
            }
            continue;
        }
        match raw.as_bytes().first().copied() {
            None => {}
            Some(b'@') if raw.starts_with("@@") => {}
            Some(b'\\') => {}
            Some(b'+') => {
                let p = new_path.as_deref().unwrap_or("");
                if counts_toward_depth(p) && is_logical_line(p, &raw[1..]) {
                    file_logical += 1;
                }
            }
            Some(b'-') => {
                let p = old_path.as_deref().unwrap_or("");
                if counts_toward_depth(p) && is_logical_line(p, &raw[1..]) {
                    file_logical += 1;
                }
            }
            Some(b' ') => {}
            Some(_) => in_hunk = false,
        }
    }
    if file_logical > 0 {
        files += 1;
        lloc += file_logical;
    }
    (files, lloc)
}

/// PURE. The slice reference-file path, UNDER `.git/` so it survives `git reset --hard && git clean -fdq`,
/// never dirties the worktree, never blocks `--resume`, and is invisible to the write-capable fix turn.
pub fn slice_ref_path(clone: &Path, runid: &str) -> PathBuf {
    clone
        .join(".git/a2a-bridge/review-slices")
        .join(format!("slice-{runid}.md"))
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

/// PURE. Reduce drained workflow events → (completed, terminal_output, failed_reviewer_legs). A failed
/// non-`synth` node is a degraded reviewer leg; draft + refine of the same leg dedup (strip `_draft`).
pub fn reduce(events: &[bridge_workflow::executor::WorkflowEvent]) -> (bool, String, usize) {
    use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
    let (mut completed, mut output) = (false, String::new());
    let mut failed_legs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in events {
        match e {
            WorkflowEvent::NodeFinished { node, ok, .. } if !ok && node.as_str() != "synth" => {
                let id = node.as_str();
                let leg = id.strip_suffix("_draft").unwrap_or(id);
                failed_legs.insert(leg.to_string());
            }
            WorkflowEvent::Terminal { outcome, output: o } => {
                completed = matches!(outcome, WorkflowOutcome::Completed);
                output = o.clone();
            }
            _ => {}
        }
    }
    (completed, output, failed_legs.len())
}

/// PURE. The `{{input}}` the reviewers + synth see: the task + both host-resolved SHAs + the explicit
/// instruction to diff + navigate. The diff is NOT inlined — reviewers run `git diff` in the clone.
/// Pass `slice_ref` to point reviewers at a prism review-slice file under `.git/a2a-bridge/review-slices/`.
pub fn build_review_input(
    task: &str,
    base_sha: &str,
    head_sha: &str,
    slice_ref: Option<&str>,
) -> String {
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
        let i = build_review_input(
            "do X",
            "aaa",
            "bbb",
            Some(".git/a2a-bridge/review-slices/slice-1.md"),
        );
        assert!(i.contains("prism review-slice") && i.contains("slice-1.md"));
    }

    #[test]
    fn slice_ref_path_lives_under_git_a2a_bridge() {
        let p = slice_ref_path(std::path::Path::new("/clone"), "task-7-1");
        assert_eq!(
            p,
            std::path::PathBuf::from("/clone/.git/a2a-bridge/review-slices/slice-task-7-1.md")
        );
    }

    #[test]
    fn counts_toward_depth_excludes_docs_tests_locks_generated() {
        assert!(counts_toward_depth("src/main.rs"));
        assert!(counts_toward_depth("deploy/Containerfile"));
        assert!(counts_toward_depth("config/app.toml"));
        // ambiguous dirs still count (bias toward review)
        assert!(counts_toward_depth("src/build/mod.rs"));
        assert!(counts_toward_depth("src/gen/codegen.rs"));
        // excluded
        assert!(!counts_toward_depth("docs/x.md"));
        assert!(!counts_toward_depth("README.markdown"));
        assert!(!counts_toward_depth("tests/it.rs"));
        assert!(!counts_toward_depth("src/foo_test.go"));
        assert!(!counts_toward_depth("pkg/api.test.ts"));
        assert!(!counts_toward_depth("Cargo.lock"));
        assert!(!counts_toward_depth("frontend/yarn.lock"));
        assert!(!counts_toward_depth("go.sum"));
        assert!(!counts_toward_depth("web/app.min.js"));
        assert!(!counts_toward_depth("api/user.pb.go"));
        assert!(!counts_toward_depth("node_modules/x/index.js"));
        assert!(!counts_toward_depth("vendor/lib/x.go"));
    }

    #[test]
    fn is_logical_line_is_path_aware() {
        // Rust: attributes are CODE (not comments); // is a comment.
        assert!(is_logical_line("src/a.rs", "#[derive(Debug)]"));
        assert!(is_logical_line("src/a.rs", "#![allow(dead_code)]"));
        assert!(!is_logical_line("src/a.rs", "// a comment"));
        assert!(is_logical_line("src/a.rs", "    *ptr = x;")); // leading * deref counts
                                                               // toml/yaml/sh/Dockerfile: # is a comment.
        assert!(!is_logical_line("a.toml", "# c"));
        assert!(!is_logical_line("Dockerfile", "# syntax=docker"));
        assert!(!is_logical_line(
            "Containerfile",
            "# syntax=docker/dockerfile:1"
        ));
        assert!(is_logical_line("a.toml", "key = 1"));
        assert!(is_logical_line("Containerfile", "RUN echo ok"));
        // sql: -- is a comment but counts in .rs
        assert!(!is_logical_line("q.sql", "-- comment"));
        assert!(is_logical_line("src/a.rs", "x -= 1;"));
        // blanks excluded everywhere; unknown ext strips nothing.
        assert!(!is_logical_line("src/a.rs", "   "));
        assert!(is_logical_line(
            "data.bin",
            "# not stripped for unknown ext"
        ));
    }

    #[test]
    fn parse_diff_for_depth_counts_code_infra_lloc_only() {
        let patch = "\
diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,4 @@
 fn x() {}
+#[derive(Debug)]
+struct B;
+// just a comment
+
diff --git a/docs/readme.md b/docs/readme.md
--- a/docs/readme.md
+++ b/docs/readme.md
@@ -1 +1,9 @@
+lots of docs lines that must NOT count
diff --git a/Cargo.lock b/Cargo.lock
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1 +1,40 @@
+churn churn
";
        // a.rs: #[derive] + struct B count (2); comment + blank excluded. docs + lock excluded.
        assert_eq!(parse_diff_for_depth(patch), (1, 2));
    }

    #[test]
    fn parse_diff_for_depth_hunk_stateful_does_not_misread_content() {
        // A removed YAML `---` renders as `----`; an added `+++` as `++++`; must count as changed lines,
        // not be misread as file headers.
        let patch = "\
diff --git a/k8s/x.yaml b/k8s/x.yaml
--- a/k8s/x.yaml
+++ b/k8s/x.yaml
@@ -1,2 +1,2 @@
-name: a
+++++name: b
";
        // one removed `-name: a` (logical) + one added `+++++name: b` (logical) = 2 lines, 1 file.
        assert_eq!(parse_diff_for_depth(patch), (1, 2));
    }

    #[test]
    fn parse_diff_for_depth_rename_out_of_code_counts_removals() {
        // src/a.rs renamed to docs/a.md WITH an edit: the new path is excluded (docs), but the old path
        // is code, so removed lines count; the added doc line does not.
        let patch = "\
diff --git a/src/a.rs b/docs/a.md
similarity index 60%
rename from src/a.rs
rename to docs/a.md
--- a/src/a.rs
+++ b/docs/a.md
@@ -1,2 +1,1 @@
-fn gone() {}
-let removed = 1;
+now docs
";
        assert_eq!(parse_diff_for_depth(patch), (1, 2));
    }

    #[test]
    fn parse_diff_for_depth_pure_rename_and_binary_are_zero() {
        let patch = "\
diff --git a/src/a.rs b/src/b.rs
similarity index 100%
rename from src/a.rs
rename to src/b.rs
diff --git a/logo.png b/logo.png
index 1..2 100644
Binary files a/logo.png and b/logo.png differ
";
        assert_eq!(parse_diff_for_depth(patch), (0, 0));
    }

    #[test]
    fn parse_diff_for_depth_quoted_md_path_excluded() {
        let patch = "\
diff --git \"a/docs/a b.md\" \"b/docs/a b.md\"
--- \"a/docs/a b.md\"
+++ \"b/docs/a b.md\"
@@ -1 +1,2 @@
+a doc line
";
        assert_eq!(parse_diff_for_depth(patch), (0, 0));
    }

    #[test]
    fn select_tier_three_way() {
        // signature: (files, lines, light_max_lines, light_max_files, thorough_min_lines, thorough_min_files)
        // light: both under
        assert_eq!(select_tier(2, 15, 15, 2, 150, 6), Tier::Light);
        // standard: in the band
        assert_eq!(select_tier(3, 16, 15, 2, 150, 6), Tier::Standard);
        assert_eq!(select_tier(5, 149, 15, 2, 150, 6), Tier::Standard);
        // thorough by lines OR files
        assert_eq!(select_tier(1, 150, 15, 2, 150, 6), Tier::Thorough);
        assert_eq!(select_tier(6, 10, 15, 2, 150, 6), Tier::Thorough);
    }

    #[test]
    fn resolve_forced_wins_and_unknown_is_standard() {
        // Forced always wins, even with no sizing.
        assert_eq!(
            Depth::Forced(Tier::Thorough).resolve(None, 15, 2, 150, 6),
            Tier::Thorough
        );
        assert_eq!(
            Depth::Forced(Tier::Light).resolve(Some((9, 9)), 15, 2, 150, 6),
            Tier::Light
        );
        // Auto + Some sizes; Auto + None (git/parse failure) → Standard fail-safe (NOT Thorough).
        assert_eq!(
            Depth::Auto.resolve(Some((0, 0)), 15, 2, 150, 6),
            Tier::Light
        );
        assert_eq!(Depth::Auto.resolve(None, 15, 2, 150, 6), Tier::Standard);
    }

    #[test]
    fn tier_workflow_suffix_maps_all_tiers() {
        assert_eq!(tier_workflow_suffix(Tier::Light), "-light");
        assert_eq!(tier_workflow_suffix(Tier::Standard), "");
        assert_eq!(tier_workflow_suffix(Tier::Thorough), "-thorough");
    }

    #[test]
    fn depth_parse_flag_and_forced_str_round_trip() {
        assert_eq!(Depth::parse_flag("auto"), Ok(Depth::Auto));
        assert_eq!(Depth::parse_flag("light"), Ok(Depth::Forced(Tier::Light)));
        assert_eq!(
            Depth::parse_flag("standard"),
            Ok(Depth::Forced(Tier::Standard))
        );
        assert_eq!(
            Depth::parse_flag("thorough"),
            Ok(Depth::Forced(Tier::Thorough))
        );
        assert!(Depth::parse_flag("bogus").is_err());

        assert_eq!(
            Depth::from_forced_str(Some("thorough")),
            Depth::Forced(Tier::Thorough)
        );
        assert_eq!(Depth::from_forced_str(None), Depth::Auto);
        assert_eq!(Depth::from_forced_str(Some("bogus")), Depth::Auto);

        assert_eq!(
            Depth::Forced(Tier::Thorough).to_forced_str(),
            Some("thorough".to_string())
        );
        assert_eq!(Depth::Auto.to_forced_str(), None);
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

    #[test]
    fn reduce_dedups_draft_and_refine_into_one_leg() {
        use bridge_core::ids::NodeId;
        use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
        let fail = |id: &str| WorkflowEvent::NodeFinished {
            node: NodeId::parse(id).unwrap(),
            ok: false,
            output: String::new(),
        };
        let term = WorkflowEvent::Terminal {
            outcome: WorkflowOutcome::Completed,
            output: "VERDICT: APPROVE".into(),
        };
        // codex leg collapses at the DRAFT only (refine then "succeeds" on garbage) → still 1 failed leg.
        let ev = vec![fail("reviewer_codex_draft"), term.clone()];
        assert_eq!(reduce(&ev).2, 1);
        // both draft + refine fail → deduped to 1.
        let ev2 = vec![
            fail("reviewer_codex_draft"),
            fail("reviewer_codex"),
            term.clone(),
        ];
        assert_eq!(reduce(&ev2).2, 1);
        // standard: two distinct reviewer ids → 2; a synth failure never counts.
        let ev3 = vec![
            fail("reviewer_codex"),
            fail("reviewer_claude"),
            fail("synth"),
            term,
        ];
        assert_eq!(reduce(&ev3).2, 2);
    }
}
