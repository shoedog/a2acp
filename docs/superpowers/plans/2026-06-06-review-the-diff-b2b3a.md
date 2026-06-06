# B2b-3a — Review-the-Diff → APPROVE/REJECT — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After `implement` commits + verifies, run a 2-reviewer (codex+claude) review of the committed diff → a `synth` node emits `VERDICT: APPROVE|REJECT`, surfaced in the operator hand-off (advisory, bounded, never blocks).

**Architecture:** A new pure `review.rs` (mirrors `verify.rs`) parses the synth verdict; a `[review]` config block (mirrors `[verify]`) names the `implement-review` workflow (2 nodes share a folded reviewer prompt → a synth sink). The `Action::Commit` arm runs the review workflow on the clone (`session_cwd=clone`, bounded by a timeout) AFTER verify and appends the verdict. The post-commit tail is made infallible (precompute `clone_cwd` pre-commit; best-effort post-commit checks) so the hand-off always prints.

**Tech Stack:** Rust (bin/a2a-bridge), the existing workflow executor + review machinery, Docker/Podman.

**Spec:** `docs/superpowers/specs/2026-06-06-review-the-diff-b2b3a-design.md` (rev3, dual-reviewed).

**Conventions:** TDD green-per-task; task/code commits NO trailer (the ADR doc has it). Coverage after `cargo llvm-cov clean --workspace` (floors per ci.yml: workspace 85, bridge-core 90, bridge-acp 90, bridge-api 90, bridge-workflow 90 — new code is in the `a2a-bridge` bin → workspace 85). Review = containerized dogfood (leak-safe post-reaper) + a2a-local codex backstop.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `bin/a2a-bridge/src/review.rs` | pure `Verdict`/`ReviewOutcome`/`parse_verdict`/`build_review_input`/`outcome_suffix` | Create |
| `bin/a2a-bridge/src/config.rs` | `ReviewToml`/`ReviewConfig` (parsed `WorkflowId` + timeout + bound) + `RegistryConfig.review` | Modify |
| `bin/a2a-bridge/src/main.rs` | `mod review;`; precompute `clone_cwd`; infallible post-commit tail; the timed review stage + drain | Modify (`implement_cmd`) + `INIT_PROMPTS`/`INIT_WORKFLOWS` |
| `prompts/review-implement.md` | folded 3-dimension reviewer prompt (shared by both reviewer nodes) | Create |
| `prompts/review-implement-synth.md` | synth merge + verdict-threshold rule + the strict tail footer | Create |
| `examples/a2a-bridge.containerized.toml` | the `implement-review` workflow + `[review] workflow="implement-review"` | Modify |
| `docs/adr/0022-review-the-diff.md` | the increment's ADR | Create (trailer) |

---

## Task 1: pure `review.rs` (Verdict, parse_verdict tail-anchored, build_review_input, outcome_suffix)

**Files:** Create `bin/a2a-bridge/src/review.rs`.

- [ ] **Step 1: Write the file + tests**

```rust
//! The `implement` review-the-diff step: after commit+verify, a 2-reviewer (codex+claude) workflow reviews
//! the committed diff and a synth node emits a `VERDICT: APPROVE|REJECT` footer. This module is the PURE
//! verdict classification + hand-off suffix (mirrors verify.rs); the workflow RUN is impure (live-gated).

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

/// PURE. Tail-anchored footer parse. The synth must END with `VERDICT: APPROVE|REJECT` (+ optional
/// `SUMMARY:`). Rules: the verdict is read from the FOOTER (the last non-blank lines); if MORE THAN ONE
/// distinct `^VERDICT:` line exists anywhere → Inconclusive (a body-quoted APPROVE can't override a real
/// REJECT); an unrecognized/absent token → Inconclusive. NEVER returns Approve unless an unambiguous
/// `VERDICT: APPROVE` is the footer verdict. SUMMARY = the line immediately following the footer VERDICT
/// line iff it matches `^\s*SUMMARY:`.
pub fn parse_verdict(synth: &str) -> (Verdict, String) {
    fn starts_ci(l: &str, kw: &str) -> bool {
        let t = l.trim_start();
        t.len() >= kw.len() && t[..kw.len()].eq_ignore_ascii_case(kw)
    }
    let lines: Vec<&str> = synth.lines().collect();
    // Exactly ONE `VERDICT:` line anywhere — 0 or conflicting (>=2) → Inconclusive (a body-quoted
    // line-start VERDICT can't override a real one).
    let vidxs: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| starts_ci(l, "VERDICT:")).map(|(i, _)| i).collect();
    if vidxs.len() != 1 {
        return (Verdict::Inconclusive, String::new());
    }
    let vi = vidxs[0];
    // TAIL-ANCHORED: after the VERDICT line, allow ONLY an immediately-following SUMMARY line, then blanks.
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
            WorkflowEvent::NodeFinished { node, ok, .. } if !ok && node.as_str() != "synth" => failed += 1,
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
        // TWO line-start VERDICT: lines (e.g. a fenced quote) → conflict → Inconclusive
        let s = "```\nVERDICT: APPROVE\n```\n\nVERDICT: REJECT\nSUMMARY: missing X";
        assert_eq!(parse_verdict(s).0, Verdict::Inconclusive);
    }

    #[test]
    fn garbage_token_is_inconclusive_never_approve() {
        assert_eq!(parse_verdict("VERDICT: maybe").0, Verdict::Inconclusive);
    }

    #[test]
    fn finding_mentioning_approve_does_not_match() {
        // only a line STARTING with VERDICT: counts; the mid-line mention is ignored
        let s = "MAJOR: the author wants to approve this quickly\n\nVERDICT: REJECT";
        assert_eq!(parse_verdict(s).0, Verdict::Reject);
    }

    #[test]
    fn footer_not_at_tail_is_inconclusive() {
        // a VERDICT followed by more findings (not the footer) → Inconclusive (tail-anchored)
        let s = "VERDICT: APPROVE\nMINOR: one more thing";
        assert_eq!(parse_verdict(s).0, Verdict::Inconclusive);
    }

    #[test]
    fn non_adjacent_summary_breaks_the_footer() {
        // a SUMMARY after a blank line is non-footer trailing content → Inconclusive (fail-safe)
        let s = "VERDICT: APPROVE\n\nSUMMARY: not adjacent";
        assert_eq!(parse_verdict(s).0, Verdict::Inconclusive);
    }

    #[test]
    fn reduce_counts_failed_reviewers_and_terminal() {
        use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
        use bridge_core::ids::NodeId;
        let ev = vec![
            WorkflowEvent::NodeFinished { node: NodeId::parse("reviewer_codex").unwrap(), ok: false, output: String::new() },
            WorkflowEvent::NodeFinished { node: NodeId::parse("reviewer_claude").unwrap(), ok: true, output: "ok".into() },
            WorkflowEvent::NodeFinished { node: NodeId::parse("synth").unwrap(), ok: true, output: "VERDICT: APPROVE".into() },
            WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output: "VERDICT: APPROVE\nSUMMARY: ok".into() },
        ];
        let (completed, output, failed) = reduce(&ev);
        assert!(completed);
        assert_eq!(failed, 1); // the codex reviewer leg failed; synth failure would NOT count here
        assert!(output.contains("VERDICT: APPROVE"));
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
}
```

- [ ] **Step 2: Wire the module + run tests**

Add `mod review;` near `mod verify;` in `bin/a2a-bridge/src/main.rs`.
Run: `cargo test -p a2a-bridge --bin a2a-bridge review::`
Expected: PASS (all `review::` unit tests — parse_verdict matrix incl. tail-anchoring + conflict + footer-not-at-tail, `reduce`, `build_review_input`, `outcome_suffix`).

- [ ] **Step 3: Commit**

```bash
git add bin/a2a-bridge/src/review.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(b2b3a): pure review.rs (tail-anchored parse_verdict, build_review_input, outcome_suffix)"
```

---

## Task 2: `[review]` config (parsed WorkflowId + timeout + bound)

**Files:** Modify `bin/a2a-bridge/src/config.rs`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in config.rs (mirror the `verify_config_*` suite; include `[server]`):

```rust
#[test]
fn review_config_parses_workflow_and_defaults() {
    let c = RegistryConfig::parse(
        "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n[review]\nworkflow=\"implement-review\"\n",
    )
    .unwrap();
    let r = c.review.as_ref().unwrap().to_config().unwrap();
    assert_eq!(r.workflow.as_str(), "implement-review");
    assert_eq!(r.max_output_bytes, 16 * 1024);
    assert_eq!(r.timeout, std::time::Duration::from_secs(300));
}

#[test]
fn review_config_defaults_workflow_when_absent_block_field() {
    let c = RegistryConfig::parse(
        "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n[review]\n",
    )
    .unwrap();
    assert_eq!(c.review.as_ref().unwrap().to_config().unwrap().workflow.as_str(), "implement-review");
}

#[test]
fn review_config_rejects_malformed_workflow_id() {
    let c = RegistryConfig::parse(
        "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n[review]\nworkflow=\"\"\n",
    )
    .unwrap();
    assert!(c.review.as_ref().unwrap().to_config().is_err()); // empty id → WorkflowId::parse error → ConfigError
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --bin a2a-bridge review_config`
Expected: FAIL — no field `review`.

- [ ] **Step 3: Implement the types**

Add near `VerifyToml` in config.rs:

```rust
fn default_review_workflow() -> String {
    "implement-review".to_string()
}

#[derive(Debug, serde::Deserialize)]
pub struct ReviewToml {
    #[serde(default = "default_review_workflow")]
    pub workflow: String,
    #[serde(default)]
    pub max_output_bytes: Option<usize>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ReviewConfig {
    pub workflow: bridge_core::ids::WorkflowId,
    pub max_output_bytes: usize,
    pub timeout: std::time::Duration,
}

impl ReviewToml {
    pub fn to_config(&self) -> Result<ReviewConfig, ConfigError> {
        let workflow = bridge_core::ids::WorkflowId::parse(self.workflow.clone())
            .map_err(|e| ConfigError::Registry(format!("[review] workflow id: {e:?}")))?;
        let max_output_bytes = self.max_output_bytes.filter(|&n| n > 0).unwrap_or(16 * 1024);
        let timeout = std::time::Duration::from_secs(self.timeout_secs.unwrap_or(300));
        Ok(ReviewConfig {
            workflow,
            max_output_bytes,
            timeout,
        })
    }
}
```

Add to `RegistryConfig` (beside `verify`):
```rust
    #[serde(default)]
    pub review: Option<ReviewToml>,
```
(Confirm `WorkflowId::parse` takes `impl Into<String>` — pass `self.workflow.clone()`, NOT `.into()` per the B2b-1 gotcha. Confirm `WorkflowId::as_str()` exists for the tests.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --bin a2a-bridge review_config`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(b2b3a): [review] config (parsed WorkflowId, timeout, output bound)"
```

---

## Task 3: the `implement-review` workflow + 2 prompts (embedded + example)

**Files:** Create `prompts/review-implement.md`, `prompts/review-implement-synth.md`; modify `bin/a2a-bridge/src/main.rs` (`INIT_PROMPTS`, `INIT_WORKFLOWS`) + `examples/a2a-bridge.containerized.toml`.

- [ ] **Step 1: Write `prompts/review-implement.md`** (the folded reviewer, shared by both nodes)

```markdown
You are ONE of two INDEPENDENT reviewers of a committed code change. Another reviewer (a different model)
reviews it in parallel; a synthesizer merges your two reviews. Cover all three dimensions below; lean into
YOUR model's strength (correctness/blockers, or architecture/design — whichever you are stronger at).

READ-ONLY + BOUNDED CONTRACT — follow exactly:
- You MAY use READ-ONLY tools: read files, list dirs, grep/search, and `git diff`/`git log`/`git show`.
- Read ONLY within this repository (your working directory). Do NOT read outside it.
- You may NOT modify anything: no edit/write/create/delete, no builds, formatters, installs, test runs, or
  any network/shell beyond the read-only git/search above. When your review is complete, STOP.

REVIEW — assess the committed change against the TASK below, using `git diff` + navigation of the repo:
1. ACCEPTANCE — does the change DELIVER the task (incl. requirements the task implies)? Call out gaps,
   missing requirements, and cases the task implies but the diff ignores.
2. CORRECTNESS — bugs, regressions, edge-cases, broken invariants, tests that don't actually test.
3. DESIGN — architecture/pattern fit: right module/layer, no needless duplication, no boundary violations.

OUTPUT: a prioritized list, each finding tagged **BLOCKER / MAJOR / MINOR** with location + the fix.
End with a one-line overall assessment. (Do NOT emit a VERDICT line — the synthesizer decides the verdict.)

{{input}}
```

- [ ] **Step 2: Write `prompts/review-implement-synth.md`** (merge + verdict)

```markdown
Synthesize ONE merged review + a VERDICT from the two independent reviews below.

OUTPUT CONTRACT — follow exactly:
- Respond with the merged review as plain text ONLY, directly in this reply. Do NOT use tools/read/write/run.
- De-duplicate; keep each reviewer's strongest unique points (one leans correctness, one architecture).
  If a reviewer reported an error marker instead of a review (its node failed), note the lens is missing
  and synthesize from the surviving reviewer.

VERDICT RULE — decide deterministically:
- REJECT if ANY of: a BLOCKER finding; the change does NOT deliver the task (acceptance unmet — regardless
  of how a reviewer tagged it); or a correctness MAJOR that means the change is wrong/unsound.
- Otherwise APPROVE (MINOR / style issues do not block — note them in the summary).

OUTPUT FORMAT: the prioritized merged findings (BLOCKER → MAJOR → MINOR), THEN end with EXACTLY these two
final lines and NOTHING after them:
VERDICT: APPROVE
SUMMARY: <one line: why, and the top issue if any>
(use `VERDICT: REJECT` instead when the rule says reject.)

=== REVIEWER A (default: codex — leans correctness) ===
{{reviewer_codex}}

=== REVIEWER B (default: claude — leans architecture) ===
{{reviewer_claude}}

(Change under review: {{input}})
```

- [ ] **Step 3: Register in `INIT_PROMPTS` + `INIT_WORKFLOWS`** (main.rs)

Add to `INIT_PROMPTS` (the `&[(&str,&str)]` at ~1004):
```rust
    ("prompts/review-implement.md", include_str!("../../../prompts/review-implement.md")),
    ("prompts/review-implement-synth.md", include_str!("../../../prompts/review-implement-synth.md")),
```
Append to the `INIT_WORKFLOWS` TOML string (~1083), after `code-review`:
```toml

[[workflows]]
id = "implement-review"
[[workflows.nodes]]
id = "reviewer_codex"
agent = "codex"
prompt_file = "prompts/review-implement.md"
inputs = []
[[workflows.nodes]]
id = "reviewer_claude"
agent = "claude"
prompt_file = "prompts/review-implement.md"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "prompts/review-implement-synth.md"
inputs = ["reviewer_codex", "reviewer_claude"]
```

- [ ] **Step 4: Add to the example config** (`examples/a2a-bridge.containerized.toml`)

Add the same `[[workflows]] id="implement-review"` block (with `../prompts/...` paths) next to `code-review`, and add a top-level `[review]` block near `[verify]`:
```toml
[review]
workflow = "implement-review"
# timeout_secs = 300   # bound; absent → 300
```

- [ ] **Step 5: Update the init-count assertion (4→5) + verify loads**

`init_generated_config_parses_and_loads` (main.rs ~1710-1729) **hard-asserts `wf.len() == 4`** with a
message naming the four workflows; `INIT_WORKFLOWS` is emitted when codex+claude are both selected (the
test selects both), so adding `implement-review` makes it **5**. Update the assertion `4 → 5` and add
`implement-review` to the message string (a certain break otherwise). (`reference_multi_agent_config_…`
reads a separate untouched file; `containerized_config_validates_with_implement_edit` uses `contains_key`,
not `len`, so it's unaffected.)

Run:
```bash
cargo test -p a2a-bridge --bin a2a-bridge init_generated_config_parses_and_loads
cargo test -p a2a-bridge --bin a2a-bridge containerized_config_validates_with_implement_edit   # example config still loads with [review] + implement-review
cargo run -q -p a2a-bridge -- run-workflow __nope__ --input README.md --config examples/a2a-bridge.containerized.toml 2>&1 | grep -q "unknown workflow" && echo "example deserializes"
```
Expected: both tests pass; `example deserializes`. NOTE: `[review].to_config()` validation on the real
example runs on the `implement` path — covered by Task 2's unit tests + the Task 5 live gate, not here.

- [ ] **Step 6: Hand-iterate the prompts (optional, operator)**

`target/debug/a2a-bridge run-workflow implement-review --input <a built review input> --session-cwd <a clone> --config examples/a2a-bridge.containerized.toml` → eyeball the synth footer. No implement change yet.

- [ ] **Step 7: Commit**

```bash
git add prompts/review-implement.md prompts/review-implement-synth.md bin/a2a-bridge/src/main.rs examples/a2a-bridge.containerized.toml
git commit -m "feat(b2b3a): implement-review workflow + folded reviewer/synth prompts (embedded + example)"
```

---

## Task 4: integrate into `implement_cmd` (precompute clone_cwd; infallible post-commit; timed review)

**Files:** Modify `bin/a2a-bridge/src/main.rs` (`implement_cmd`).

- [ ] **Step 1: Precompute `clone_cwd` pre-commit + capture `review_cfg`**

After `let pre = implement::head_sha(&clone)?;` (~main.rs:496), add (fallible HERE is fine — pre-commit):
```rust
    // Precompute the clone's SessionCwd ONCE (pre-commit) — reused by the implement-edit ctx, verify, and
    // review so NO `SessionCwd::parse` runs after the commit (the hand-off must always print).
    let clone_cwd = bridge_core::SessionCwd::parse(&clone.to_string_lossy())?;
```
Beside `verify_cfg` (~516):
```rust
    let review_cfg = cfg.review.as_ref().map(|t| t.to_config());
```
Replace the implement-edit ctx parse (~537) `session_cwd: Some(bridge_core::SessionCwd::parse(&clone.to_string_lossy())?)` with `session_cwd: Some(clone_cwd.clone())`. Replace the verify-arm parse (~624) `let clone_cwd = bridge_core::SessionCwd::parse(&clone.to_string_lossy())?;` with `let clone_cwd = clone_cwd.clone();` (or just use the outer `clone_cwd`).

- [ ] **Step 2: Make the post-commit `stage_state` check best-effort**

The post-commit dirty-note check (~604) `implement::stage_state(&clone).map_err(|e| format!("implement: post-commit stage: {e}"))?` → best-effort (no `?`):
```rust
            if !matches!(
                implement::stage_state(&clone).unwrap_or(implement::StageState::Clean),
                implement::StageState::Clean
            ) {
                eprintln!("[implement] note: the clone still has uncommitted changes the agent left unstaged.");
            }
```

- [ ] **Step 3: Add a review-drain helper** (captures output + counts failed reviewer legs)

Add a free async fn in main.rs (near `implement_cmd`). `run_with_context` returns the
`bridge_workflow::executor::WorkflowStream` alias (= `Pin<Box<dyn Stream<Item=Result<WorkflowEvent,
BridgeError>>+Send>>`), so take it DIRECTLY — do NOT `Box::pin` it (that double-pins → type mismatch). It
collects events then delegates the reduction to the PURE `review::reduce`:
```rust
/// Drain a review workflow stream → (completed, synth_output, reviewers_failed). Collects events + delegates
/// the reduction to the pure `review::reduce`. Keeps polling to the end so the executor runs its cancel
/// cleanup (backend.cancel/forget_session) even after a timeout cancel.
async fn drain_review(mut stream: bridge_workflow::executor::WorkflowStream) -> (bool, String, usize) {
    use futures::StreamExt;
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(ev) => events.push(ev),
            Err(e) => eprintln!("[implement] review: stream error: {e:?}"),
        }
    }
    review::reduce(&events)
}
```

- [ ] **Step 4: Add the timed review stage in the `Action::Commit` arm** (after the verify suffix, before `println!`)

Replace `handoff.push_str(&verify::outcome_suffix(&outcome)); \n println!("{handoff}");` (~656-658) with the verify suffix PLUS the review stage (all infallible — no `?`):
```rust
            handoff.push('\n');
            handoff.push_str(&verify::outcome_suffix(&outcome));

            // B2b-3a: advisory review of the committed diff (bounded; never blocks the hand-off).
            let review_outcome = match review_cfg {
                None => review::ReviewOutcome::NotConfigured,
                Some(Err(e)) => {
                    eprintln!("[implement] review: config error: {e:?}");
                    review::ReviewOutcome::ConfigError
                }
                Some(Ok(rcfg)) => match wf_map.get(&rcfg.workflow).cloned() {
                    None => review::ReviewOutcome::NotLoaded,
                    Some(graph) => {
                        let input = review::build_review_input(&a.task, &base_sha, &sha);
                        let ctx = bridge_workflow::executor::WorkflowRunContext {
                            session_cwd: Some(clone_cwd.clone()),
                        };
                        let token = tokio_util::sync::CancellationToken::new();
                        let stream = executor.run_with_context(
                            graph,
                            input,
                            format!("impl-review-{task_id}"),
                            token.clone(),
                            ctx,
                        );
                        eprintln!("[implement] review: running implement-review");
                        // Timeout = cancel-then-KEEP-DRAINING (do NOT drop the stream — the executor must
                        // keep being polled to run backend.cancel/forget_session, executor.rs:136/312).
                        let mut drain = std::pin::pin!(drain_review(stream));
                        let (completed, synth, failed) = tokio::select! {
                            r = &mut drain => r,
                            _ = tokio::time::sleep(rcfg.timeout) => {
                                eprintln!("[implement] review: timed out after {:?}", rcfg.timeout);
                                token.cancel();
                                (&mut drain).await   // keep draining so the executor cleans up
                            }
                        };
                        if completed {
                            // Parse the FULL synth (truncation drops the middle → could lose a body
                            // VERDICT and flip the verdict). Truncate only for the stderr dump.
                            let (verdict, summary) = review::parse_verdict(&synth);
                            if !matches!(verdict, review::Verdict::Approve) {
                                eprintln!(
                                    "[implement] review {verdict:?}:\n{}",
                                    verify::truncate_output(&synth, rcfg.max_output_bytes)
                                );
                            }
                            review::ReviewOutcome::Ran { verdict, summary, reviewers_failed: failed }
                        } else {
                            review::ReviewOutcome::Incomplete
                        }
                    }
                },
            };
            handoff.push('\n');
            handoff.push_str(&review::outcome_suffix(&review_outcome));

            println!("{handoff}");
            Ok(())
```

- [ ] **Step 5: Build + clippy + existing tests**

Run: `cargo build -p a2a-bridge && cargo clippy -p a2a-bridge --all-targets -- -D warnings && cargo test -p a2a-bridge --bin a2a-bridge implement:: review:: verify::`
Expected: compiles, clippy clean, all pass. (The arm is impure orchestration — covered by the live gate; the pure classification is Task-1 tested.)

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(b2b3a): run implement-review after verify; bounded; verdict in the hand-off (post-commit tail infallible)"
```

---

## Task 5: live acceptance gate (operator-run, Docker/Podman)

Prereqs: egress proxies up; `a2a-toolchain`/reader images built; creds synced.

- [ ] **Step 1: APPROVE path** — `implement` a task-satisfying change on a clone of this repo → hand-off shows `verify: PASS …` + `review: APPROVE  (…)`. Commit + hand-off happen; `:ro` reviewers reaped (poll `docker ps -aq --filter name=a2a-ro-` → 0).
- [ ] **Step 2: REJECT path** — a task the change does NOT satisfy (or a buggy one) → `review: REJECT  (…)`; commit + hand-off still happen.
- [ ] **Step 3: soft cases** — a `[review].workflow` typo → `review: skipped (unknown workflow)` + exit 0; set `timeout_secs=1` on a real run → `review: incomplete (did not finish)` + exit 0 + the `:ro` reviewers reaped (cancel).
- [ ] **Step 4: degradation** — force a reviewer leg to fail (bad agent/transport) → suffix shows `[1 reviewer(s) failed]`, synth still emits a verdict.
- [ ] **Step 5: post-commit infallibility** — confirm the commit + hand-off ALWAYS print across Steps 1-4 (no abort after commit). Record results in ADR-0022.

---

## Task 6: ADR-0022

**Files:** Create `docs/adr/0022-review-the-diff.md`.

- [ ] **Step 1:** Write the ADR — the decision (advisory bounded review of the committed diff; Topology B; verdict thresholds; post-commit-infallible), the design cross-check + dual-review folds (post-commit `?` blocker incl the latent B2b-2 fix, verdict thresholds, tail-anchored parse, outcome taxonomy, timeout), the live-gate result, deferred (B2b-3b loop, adaptive depth, code-nav tooling, originator routing, spec-file input). End with the trailer.
- [ ] **Step 2:** Commit with `-m` flags (trailer as the last `-m`).

---

## Final verification

- [ ] `cargo fmt --all`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` → clean
- [ ] `cargo llvm-cov clean --workspace` then floors per ci.yml (workspace 85, bridge-core/acp/api/workflow 90); new code is the `a2a-bridge` bin (review.rs well-tested + config) → workspace 85
- [ ] the Task-5 live gate PASS recorded in ADR-0022
- [ ] Use **superpowers:finishing-a-development-branch** (merge to main, then push)

## Dual-review folds (plan rev2)

Both plan reviews (containerized + a2a-local codex) confirmed the seams (WorkflowStream type, post-commit
infallibility, config, embedded shape) and drove: **tail-anchored `parse_verdict`** (exactly-one VERDICT,
must be the footer, conflict/footer-not-at-tail→Inconclusive) + fixed fixtures + a `footer_not_at_tail`
test; a pure **`reduce(events)`** extracted from `drain_review` + unit-tested; **timeout = `select!`-cancel-
then-keep-draining** (not drop — the executor needs polling to run its cancel cleanup); **parse the FULL
synth** (truncate only the stderr dump) + **`eprintln` the body on non-APPROVE** (surface the findings, not
just the one-liner); `drain_review` takes `WorkflowStream` (no `Box::pin`); **init-count 4→5** (explicit).

## Self-review (spec coverage)

- Topology B workflow (2 folded reviewers → synth) → Task 3. Acceptance/correctness/design folded in the reviewer prompt + the verdict-threshold rule in synth → Task 3. Tail-anchored `parse_verdict` + never-infer-APPROVE + conflicting→Inconclusive → Task 1. `[review]` parsed `WorkflowId` (infallible post-commit lookup) + timeout + bound → Task 2. Post-commit infallible (precompute `clone_cwd`, best-effort stage, latent B2b-2 `?` removed) + bounded timeout + full outcome taxonomy + degradation suffix → Task 4. base/head SHA binding → Task 1 `build_review_input` + Task 4 (`base_sha`/`sha`). Embedded + example registration → Task 3. Live: APPROVE/REJECT/soft/timeout/degradation/always-commit → Task 5.
