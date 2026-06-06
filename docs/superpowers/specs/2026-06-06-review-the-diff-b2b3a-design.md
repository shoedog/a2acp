# B2b-3a — Review-the-Diff → APPROVE/REJECT — Design

**Date:** 2026-06-06
**Status:** Draft (rev2 — folds the firewalled clean-room `design`-workflow cross-check + owner decisions:
Topology B (2 folded diverse reviewers), model-is-agent-level, reuse/embeds, refined outcome taxonomy).
**Builds on:** B2b-1 (`implement`, ADR-0019), B2b-2 (verify, ADR-0020), the `:ro` reaper (ADR-0021 — its
prerequisite: review spawns `:ro` lenses, now reaped). B2b-3b (the review→tweak loop) follows.

## Goal

After `implement` commits the agent's change (and B2b-2 verifies it), run a **multi-agent review of the
committed diff** and surface an **APPROVE/REJECT** verdict in the operator hand-off — **advisory** (the
operator/originator makes the final accept at merge). Flow:
`edit → commit → verify → review-the-diff → hand-off-with-verdict`.

## Decisions (settled with the owner + the clean-room cross-check)

1. **Topology B — two diverse reviewers, all dimensions folded, → synth.** Both reviewers (codex + claude)
   review the diff for **acceptance** (does it DELIVER the task/spec?), **correctness** (bugs/regressions/
   edge-cases), and **design** (architecture fit) in one pass each, weighted to the model's strength; a
   synth node merges them and emits the verdict. Rationale: the dominant quality lever is *independent
   diverse reviewers*, and the bridge has exactly two model families today (codex=gpt-5.5; claude=
   subscription; no sonnet agent) — so diversity caps at 2 regardless of lens count. Two broad diverse
   reviewers keep full diversity at **3 calls** (2 reviewers ‖ + synth) vs 4 for separate lenses; acceptance
   is then covered by **both** models. (3-focused-lens and 1-reviewer variants are alternate workflow
   definitions; **adaptive depth** — pick the workflow by `git diff --stat` — is a deferred fast-follow.)
2. **Acceptance is first-class** — both reviewer prompts make dimension #1 "does the change DELIVER the
   task/spec? (gaps, missing requirements, cases the task implies)"; the synth `APPROVE` ⟺ delivers the
   task **and** correct **and** sound. (Final acceptance is the **originator's** — the operator's hand-off +
   merge; routing acceptance to a dispatching orchestrator is deferred.)
3. **Model tier is an AGENT-level property, not a node knob.** `WorkflowNodeToml` has only
   `id`/`agent`/`prompt_file`/`inputs`; a node's model = which `[[agents]]` entry it routes to. So `[review]`
   only NAMES a workflow id (no model fields); to retune a reviewer's model you point its node at a
   different agent entry. (A mid-tier/sonnet agent, if wanted, is a config prerequisite, not part of this
   slice — `load_workflows` fails loud at boot on a missing agent.)
4. **Reviewers navigate the CLONE with tools** (read access + read-only git/grep/search), not a diff in
   isolation — richer context finds more defects (owner). The `:ro` reviewers run with `session_cwd =
   clone`, bounded by the read-only prompt contract + the `:ro` container. The bridge passes the task + the
   host-resolved `base_sha`/`head_sha` and the instruction to `git diff <base>..<head>`; the full diff is
   NOT inlined (avoids prompt blow-up; reviewers navigate). Richer code-nav (tree-sitter/prism/LSP/symbol
   tools) is **deferred** (image + tooling enhancement).
5. **Machine-parseable verdict, fail-safe.** Synth ends with EXACTLY:
   `VERDICT: APPROVE` (or `REJECT`) then `SUMMARY: <one-line reason>`. `parse_verdict` scans for the LAST
   `^\s*VERDICT:\s*(APPROVE|REJECT)\b` (case-insensitive, last-wins so trailing prose/quoted alternatives
   can't fool it); anything else → `Inconclusive`. **NEVER infer APPROVE.** It also lifts an adjacent
   `SUMMARY:` line.
6. **Advisory, not gating; never block the hand-off.** The verdict is REPORTED; `implement` always commits
   + hands off + exits 0. **Invariant: NO `?` between the commit and `println!(handoff)`** — every review
   failure degrades to a reported outcome. The review→tweak LOOP (re-prompt on REJECT/verify-FAIL) is
   **B2b-3b** and reuses `parse_verdict` unchanged.

## Architecture

### `implement-review` workflow (config + 2 prompts; reviewers run in the clone)
```toml
[[workflows]]
id = "implement-review"
[[workflows.nodes]]
id = "reviewer_codex"            # codex — leans correctness/blockers; covers accept+correct+design
agent = "codex"
prompt_file = "prompts/review-implement.md"     # NEW, folded 3-dimension, diff-native
inputs = []
[[workflows.nodes]]
id = "reviewer_claude"           # claude — leans architecture/acceptance; covers accept+correct+design
agent = "claude"
prompt_file = "prompts/review-implement.md"     # SAME folded prompt; the agent supplies the model bias
inputs = []
[[workflows.nodes]]
id = "synth"                     # merge + emit VERDICT/SUMMARY (the sink/terminal)
agent = "claude"
prompt_file = "prompts/review-implement-synth.md"   # NEW; {{reviewer_codex}} {{reviewer_claude}} {{input}}
inputs = ["reviewer_codex", "reviewer_claude"]
```
- `prompts/review-implement.md` (NEW, shared by both reviewer nodes): the READ-ONLY + BOUNDED contract
  (read/grep/`git diff`·`log`·`show`; no writes/builds/network), the `{{input}}` (TASK + base/head SHAs +
  "review `git diff <base>..<head>`, navigate the repo"), and the **three dimensions** (acceptance /
  correctness / design) with "you are ONE of two independent reviewers — cover all three, lean into your
  strength; tag findings BLOCKER/MAJOR/MINOR."
- `prompts/review-implement-synth.md` (NEW): merge `{{reviewer_codex}}` + `{{reviewer_claude}}` (de-dup,
  resolve disagreements, note a missing reviewer if a node failed), prioritized findings, then the strict
  footer (APPROVE ⟺ delivers the task AND correct/sound; a BLOCKER → REJECT).
- Registered in BOTH the embedded defaults (`INIT_WORKFLOWS`/`EMBEDDED_PROMPTS`, main.rs ~1002-1136, like
  `code-review`) AND `examples/a2a-bridge.containerized.toml`. (Embedded prompt paths are top-level
  `prompts/...`; the examples TOML uses `../prompts/...` — both coexist.)

### `[review]` config (bin/a2a-bridge/src/config.rs, beside `VerifyToml`)
```rust
pub struct ReviewToml { #[serde(default="default_review_workflow")] workflow: String,   // "implement-review"
                        #[serde(default) ] max_output_bytes: Option<usize> }            // default 16*1024
pub struct ReviewConfig { pub workflow: String, pub max_output_bytes: usize }
// RegistryConfig:  #[serde(default)] pub review: Option<ReviewToml>     // absent → step skipped
```
Parse tests mirror `verify_config_*` (absent, default workflow, default bound).

### Pure `review.rs` (bin/a2a-bridge/src/review.rs — mirrors verify.rs)
```rust
pub enum Verdict { Approve, Reject, Inconclusive }
pub enum ReviewOutcome {
    Ran { verdict: Verdict, summary: String },
    Incomplete,     // executor Terminal outcome != Completed
    NotConfigured,  // no [review]
    NotLoaded,      // [review].workflow id absent from wf_map (typo) — the ONLY soft config case
}
/// PURE. Last `^VERDICT:` line wins; case-insensitive; never infers Approve; lifts an adjacent SUMMARY.
pub fn parse_verdict(synth: &str) -> (Verdict, String);
/// PURE. The {{input}} for the reviewers: task + base/head SHAs + the `git diff <base>..<head>` + navigate.
pub fn build_review_input(task: &str, base_sha: &str, head_sha: &str) -> String;
/// PURE. One-line hand-off suffix: "review: APPROVE|REJECT  (<summary>)" / "review: inconclusive (…)" /
/// "review: incomplete (workflow did not finish)" / "review: not configured" / "review: skipped (unknown
/// workflow <id>)".
pub fn outcome_suffix(o: &ReviewOutcome) -> String;
```
Reuse `verify::truncate_output` for the stderr dump (no second truncator). The verdict classification
(`parse_verdict`/`outcome_suffix`) is the pure coverage keystone; the executor RUN is impure (live-gated).

### Integration (implement_cmd `Action::Commit` arm, AFTER the verify suffix)
- Capture `review_cfg = cfg.review.as_ref().map(|t| t.to_config())` BEFORE `into_snapshot` (beside
  `verify_cfg`); keep `wf_map` (read via `.cloned()`, not moved).
- After the verify suffix, the advisory review stage (**no `?` past the commit**):
  `None → NotConfigured`; `Some(rcfg)` → `wf_map.get(&rcfg.workflow)`: `None → NotLoaded`; else build
  `input = review::build_review_input(&a.task, &base_sha, &sha)`, **rebuild** the `WorkflowRunContext`
  (the implement-edit `ctx` was consumed by value) with `session_cwd = clone`, run
  `executor.run_with_context(graph.clone(), input, "impl-review-<task_id>", CancellationToken::new(), ctx)`,
  **drain the terminal output** (capture `WorkflowEvent::Terminal { outcome, output }` — the implement-edit
  loop discards `output`; the review loop must keep it), eprintln the truncated synth, then
  `Completed → Ran(parse_verdict(synth))` else `Incomplete`. Append `review::outcome_suffix(&outcome)` to
  the hand-off; `println!`.
- Reuses the same executor/registry; the `:ro` reviewers are reaped by the shipped reaper (their agents are
  already in `[[agents]]`, so `RoSweepGuard`'s snapshot covers them).

## Component / file boundaries

| Concern | Home |
|---|---|
| `implement-review` workflow + `review-implement.md` + `review-implement-synth.md` | embedded `INIT_WORKFLOWS`/`EMBEDDED_PROMPTS` (main.rs) + `examples/a2a-bridge.containerized.toml` + `prompts/` |
| `ReviewToml`/`ReviewConfig` + `RegistryConfig.review` | `bin/a2a-bridge/src/config.rs` |
| pure `Verdict`/`ReviewOutcome`/`parse_verdict`/`build_review_input`/`outcome_suffix` | `bin/a2a-bridge/src/review.rs` (new) |
| capture review_cfg; rebuild ctx; run; drain terminal; append suffix | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) |

## Testing
- **Unit (no Docker):** `parse_verdict` matrix — APPROVE / REJECT / missing→Inconclusive / both-present-
  last-wins / case-insensitive / leading-whitespace / `VERDICT: maybe`→Inconclusive / a finding line
  mentioning "approve" doesn't false-match (only `^VERDICT:`); SUMMARY lift; `build_review_input` asserts
  task + both SHAs + the `git diff base..head` instruction; `outcome_suffix` for each `ReviewOutcome`;
  `ReviewToml::to_config` (absent/default/explicit). The implement-arm wiring is impure (live-gated) but
  the classification is fully pure-tested.
- **Live gate (Docker, dogfooded):** `implement` a small change that satisfies the task → hand-off shows
  `verify: PASS …` + `review: APPROVE  (…)`; a change that does NOT satisfy the task (or a buggy one) →
  `review: REJECT  (…)`; a `[review].workflow` typo → `review: skipped (unknown workflow …)` and the run
  still returns Ok; assert the `:ro` reviewers are reaped (poll-to-0); the commit + hand-off ALWAYS happen.

## Deferred
- **B2b-3b:** the review→tweak loop (re-prompt an edit turn on the persistent clone on REJECT or verify-FAIL,
  re-commit/verify/review, bounded) + optional gating — reuses `parse_verdict` unchanged.
- **Adaptive review depth:** pick the review workflow by `git diff --stat` complexity (light=1 reviewer /
  standard=B / thorough=3-focused-lenses) — a thin selector + alternate workflow definitions.
- Richer review code-nav tooling (tree-sitter/prism/LSP/symbol search) in the image + exposed to the `:ro`
  reviewers.
- Routing acceptance to the **originator/dispatcher** (for orchestrated `implement`, not human-CLI); a
  spec-FILE input (beyond the task string) as the acceptance reference.

## Firewall
Designed from the bridge's own seams; **cross-checked by the bridge's own firewalled clean-room `design`
workflow** (independent of this spec — it converged on the spine + caught the model-is-agent-level fact, the
reuse/embeds, the rebuilt ctx, and the refined outcome taxonomy folded above). **Dual review = containerized
dogfood (now leak-safe post-reaper) + a2a-local `codex-review` (gpt-5.5) backstop.**
