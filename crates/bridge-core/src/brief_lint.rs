//! Dependency-free lint for operator-supplied task briefs.
//!
//! This intentionally stays heuristic: the goal is to catch recurring
//! dispatch-brief failure modes at the submit chokepoint without constraining
//! the task-spec grammar or shelling out to the historical Python validator.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum BriefLintRuleId {
    R1,
    R2,
    R3,
    R4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BriefLintSeverity {
    Warn,
    Violation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BriefLintKind {
    Implement,
    RunWorkflow,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BriefLintFinding {
    pub rule: BriefLintRuleId,
    pub name: &'static str,
    pub severity: BriefLintSeverity,
    pub message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BriefLintReport {
    pub kind: BriefLintKind,
    pub findings: Vec<BriefLintFinding>,
}

impl BriefLintReport {
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    pub fn has_violations(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == BriefLintSeverity::Violation)
    }

    pub fn rule_set(&self) -> Vec<BriefLintRuleId> {
        self.findings.iter().map(|finding| finding.rule).collect()
    }
}

pub fn lint_brief(raw: &str, kind: BriefLintKind) -> BriefLintReport {
    let lower = raw.to_lowercase();
    let mut findings = Vec::new();

    if has_premise_marker(raw, &lower) && !has_falsification_license(&lower) {
        let (line, excerpt) = first_premise_marker(raw);
        findings.push(finding(
            BriefLintRuleId::R1,
            "premise-without-license",
            BriefLintSeverity::Violation,
            "claimed conclusion/result is present without an explicit falsification license",
            line,
            excerpt,
        ));
    }

    if has_option_menu(raw, &lower) && !has_open_option_marker(&lower) {
        let (line, excerpt) = first_option_menu_marker(raw);
        findings.push(finding(
            BriefLintRuleId::R2,
            "option-menu",
            BriefLintSeverity::Warn,
            "option menu is present without a user-specified-options or open-brief marker",
            line,
            excerpt,
        ));
    }

    if kind == BriefLintKind::Implement {
        if let Some((line, excerpt)) = first_line_anchor(raw) {
            findings.push(finding(
                BriefLintRuleId::R3,
                "line-number-anchors",
                BriefLintSeverity::Warn,
                "line-number edit anchors drift as prior tasks land; prefer symbol or semantic anchors",
                Some(line),
                Some(excerpt),
            ));
        }
    }

    if let Some((line, excerpt)) = first_given_facts_without_probe(raw) {
        findings.push(finding(
            BriefLintRuleId::R4,
            "given-facts-without-probe",
            BriefLintSeverity::Violation,
            "given facts are asserted without a nearby artifact/probe reference",
            Some(line),
            Some(excerpt),
        ));
    }

    BriefLintReport { kind, findings }
}

fn finding(
    rule: BriefLintRuleId,
    name: &'static str,
    severity: BriefLintSeverity,
    message: &'static str,
    line: Option<usize>,
    excerpt: Option<String>,
) -> BriefLintFinding {
    BriefLintFinding {
        rule,
        name,
        severity,
        message,
        line,
        excerpt,
    }
}

fn has_premise_marker(raw: &str, lower: &str) -> bool {
    lower.contains("the root cause is")
        || lower.contains("all findings addressed")
        || has_claimed_test_total(lower)
        || raw.lines().any(line_has_treat_as_given)
        || lower.contains("proposed fix")
}

fn first_premise_marker(raw: &str) -> (Option<usize>, Option<String>) {
    first_line_matching(raw, |line| {
        let lower = line.to_lowercase();
        lower.contains("the root cause is")
            || lower.contains("all findings addressed")
            || has_claimed_test_total(&lower)
            || line_has_treat_as_given(line)
            || lower.contains("proposed fix")
    })
}

fn has_claimed_test_total(lower: &str) -> bool {
    lower.contains("passed") && lower.contains("0 failed")
}

fn line_has_treat_as_given(line: &str) -> bool {
    let lower = line.to_lowercase();
    let Some(treat) = lower.find("treat") else {
        return false;
    };
    let Some(as_given) = lower[treat..].find("as given") else {
        return false;
    };
    as_given > 0
}

fn has_falsification_license(lower: &str) -> bool {
    lower.contains("pressure-test")
        || lower.contains("pressure test")
        || lower.contains("independently verify")
        || lower.contains("may be wrong")
        || lower.contains("argue the opposite")
        || lower.contains("search elsewhere")
        || lower.contains("falsif")
        || lower.contains("refute")
}

fn has_option_menu(raw: &str, lower: &str) -> bool {
    let option_lines = option_like_line_count(raw);
    lower.contains("choose one")
        || lower.contains("pick one")
        || lower.contains("select one")
        || lower.contains("choose between")
        || lower.contains("pick between")
        || lower.contains("option menu")
        || (lower.contains("options:") && option_lines >= 2)
        || option_lines >= 2
}

fn first_option_menu_marker(raw: &str) -> (Option<usize>, Option<String>) {
    first_line_matching(raw, |line| {
        let lower = line.to_lowercase();
        lower.contains("choose one")
            || lower.contains("pick one")
            || lower.contains("select one")
            || lower.contains("choose between")
            || lower.contains("pick between")
            || lower.contains("options:")
            || is_lettered_option_line(line)
    })
}

fn option_like_line_count(raw: &str) -> usize {
    raw.lines()
        .filter(|line| is_lettered_option_line(line))
        .count()
}

fn is_lettered_option_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let trimmed = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .unwrap_or(trimmed)
        .trim_start();
    let lower = trimmed.to_lowercase();
    lower.starts_with("option a")
        || lower.starts_with("option b")
        || lower.starts_with("a)")
        || lower.starts_with("b)")
        || lower.starts_with("a.")
        || lower.starts_with("b.")
        || lower.starts_with("a:")
        || lower.starts_with("b:")
}

fn has_open_option_marker(lower: &str) -> bool {
    lower.contains("user-specified option")
        || lower.contains("user specified option")
        || lower.contains("user-provided option")
        || lower.contains("user provided option")
        || lower.contains("open brief")
        || lower.contains("open-ended")
        || lower.contains("open ended")
        || lower.contains("not limited to")
        || lower.contains("you may propose")
        || lower.contains("may propose another")
        || lower.contains("propose another")
        || lower.contains("other option")
        || lower.contains("none of the above")
        || lower.contains("or another")
        || lower.contains("free-form")
        || lower.contains("freeform")
}

fn first_line_anchor(raw: &str) -> Option<(usize, String)> {
    for (line_idx, line) in raw.lines().enumerate() {
        for token in line.split_whitespace() {
            let token = token.trim_matches(|c: char| "`[](){}<>,;".contains(c));
            if is_line_anchor_token(token) {
                return Some((line_idx + 1, excerpt(line)));
            }
        }
    }
    None
}

fn is_line_anchor_token(token: &str) -> bool {
    if token.contains("://") {
        return false;
    }
    let Some(colon) = token.rfind(':') else {
        return false;
    };
    let (path, digits) = token.split_at(colon);
    let digits = &digits[1..];
    if path.is_empty() || digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some(dot) = path.rfind('.') else {
        return false;
    };
    let ext = &path[dot + 1..];
    !ext.is_empty()
        && ext.len() <= 10
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && ext.chars().any(|c| c.is_ascii_alphabetic())
}

fn first_given_facts_without_probe(raw: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = raw.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        if !is_given_facts_marker(line) {
            continue;
        }
        let start = idx.saturating_sub(2);
        let end = usize::min(lines.len(), idx + 3);
        if !lines[start..end]
            .iter()
            .any(|candidate| has_probe_ref(candidate))
        {
            return Some((idx + 1, excerpt(line)));
        }
    }
    None
}

fn is_given_facts_marker(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("as given")
        || lower.contains("given data")
        || lower.contains("observed facts as established")
}

fn has_probe_ref(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("```")
        || lower.contains("$ ")
        || lower.contains("exit code")
        || lower.contains("captured at")
        || lower.contains("captured in")
        || lower.contains("captured to")
        || line.split_whitespace().any(has_artifact_path_token)
}

fn has_artifact_path_token(token: &str) -> bool {
    let token = token.trim_matches(|c: char| "`[](){}<>,;:".contains(c));
    let lower = token.to_lowercase();
    const EXTENSIONS: &[&str] = &[
        ".log", ".txt", ".md", ".json", ".jsonl", ".yaml", ".yml", ".toml", ".xml", ".html",
        ".out", ".stdout", ".stderr", ".patch", ".diff", ".rs", ".py", ".go", ".js", ".ts", ".tsx",
        ".jsx", ".java", ".c", ".cc", ".cpp", ".h", ".hpp", ".sh",
    ];
    EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

fn first_line_matching(
    raw: &str,
    mut predicate: impl FnMut(&str) -> bool,
) -> (Option<usize>, Option<String>) {
    for (idx, line) in raw.lines().enumerate() {
        if predicate(line) {
            return (Some(idx + 1), Some(excerpt(line)));
        }
    }
    (None, None)
}

fn excerpt(line: &str) -> String {
    let trimmed = line.trim();
    const MAX: usize = 160;
    let mut out: String = trimmed.chars().take(MAX).collect();
    if trimmed.chars().count() > MAX {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(raw: &str, kind: BriefLintKind) -> Vec<BriefLintRuleId> {
        lint_brief(raw, kind).rule_set()
    }

    #[test]
    fn parity_fixture_good_open_brief_has_no_hits() {
        let raw = r#"
# Investigate flaky test behavior

The current cache theory may be wrong. Independently verify the behavior,
pressure-test alternate explanations, and search elsewhere if the evidence points
outside the cache layer. This is an open brief; propose another path if needed.
"#;
        assert_eq!(rules(raw, BriefLintKind::RunWorkflow), vec![]);
    }

    #[test]
    fn parity_fixture_anchored_panel_hits_option_menu_only() {
        let raw = r#"
# Panel choice

Choose one:
A) Approve the plan as written.
B) Reject the plan as impossible.
"#;
        assert_eq!(
            rules(raw, BriefLintKind::RunWorkflow),
            vec![BriefLintRuleId::R2]
        );
    }

    #[test]
    fn parity_fixture_given_data_hits_premise_and_missing_probe() {
        let raw = r#"
# Use established observations

Treat the following observations as given. Use the given data before editing.
"#;
        assert_eq!(
            rules(raw, BriefLintKind::RunWorkflow),
            vec![BriefLintRuleId::R1, BriefLintRuleId::R4]
        );
    }

    #[test]
    fn parity_fixture_impl_line_anchors_hit_implement_only() {
        let raw = r#"
# Edit exact lines

Change bin/a2a-bridge/src/main.rs:2371 and crates/bridge-core/src/task_spec.rs:141.
"#;
        assert_eq!(
            rules(raw, BriefLintKind::Implement),
            vec![BriefLintRuleId::R3]
        );
        assert_eq!(rules(raw, BriefLintKind::RunWorkflow), vec![]);
    }

    #[test]
    fn nearby_probe_reference_suppresses_given_facts_violation() {
        let raw = r#"
Use the observed facts as established.
Captured at artifacts/run-17.json with exit code 0.
"#;
        assert_eq!(rules(raw, BriefLintKind::RunWorkflow), vec![]);
    }
}
