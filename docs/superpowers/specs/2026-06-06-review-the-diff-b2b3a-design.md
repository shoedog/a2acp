# B2b-3a — Review-the-Diff → APPROVE/REJECT — Design

**Date:** 2026-06-06
**Status:** Draft (pre dual-review).
**Builds on:** B2b-1 (`implement` clone+edit+commit, ADR-0019), B2b-2 (build+test verify, ADR-0020), the
`:ro` reaper (ADR-0021 — its prerequisite: review spawns `:ro` lenses that now get reaped). B2b-3b (the
review→tweak loop) follows.

## Goal

After `implement` commits the agent's change (and B2b-2 verifies it), run a **multi-agent review of the
committed diff** and surface an **APPROVE/REJECT verdict** in the operator hand-off — **advisory** (like
verify; the operator/originator makes the final accept at merge). Flow:
`edit → commit → verify → review-the-diff → hand-off-with-verdict`.

## Decisions (settled with the owner)

1. **Review-the-diff is a multi-agent workflow whose lenses navigate the CLONE with tools** — not a diff
   reviewed in isolation. The `:ro` lenses run with `session_cwd = clone` and their read-only toolset
   (read / grep / `git diff`·`log`·`show`, prompt-restricted, `:ro`-enforced); the bridge hands them the
   task + the base ref as the starting pointer (`git diff <base>..HEAD`). *An agent with read access +
   navigation tools finds more defects than one given only a diff* (owner). Richer code-nav (tree-sitter /
   prism / LSP / symbol tools) materially helps — **deferred** as a tooling-enhancement follow-on (add to
   the image + expose); B2b-3a uses the read-only tools available now.
2. **Acceptance is first-class** — the review explicitly checks *"does the change DELIVER the task/spec?"*,
   not just code quality. A dedicated **acceptance** lens (gaps, missing requirements, cases the task
   implies) + **correctness** (bugs/regressions/edge-cases) + **design** (architecture fit) → **synth**.
   The synth `APPROVE` ⟺ acceptance PASS **and** no correctness blocker **and** sound.
3. **Acceptance can use a cheaper model** (owner: "sonnet or gpt-codex, unless the feature is complex").
   The agent is per-node configurable; default acceptance to a cheaper agent (codex / gpt; a sonnet-backed
   agent when one exists). The *final* acceptance belongs to the **originator** (the operator who ran
   `implement`) via the hand-off + merge decision; the lens is an advisory pre-check. **Routing acceptance
   back to a dispatching orchestrator** (when `implement` is invoked inside a larger orchestration, not by a
   human at the CLI) is a **deferred** follow-on.
4. **Machine-parseable verdict.** The synth ends with a `VERDICT: APPROVE` or `VERDICT: REJECT` line
   (+ a one-line rationale + the prioritized findings). The bridge parses that line best-effort →
   `Approve | Reject | Undetermined` (unparseable → Undetermined; advisory, never blocks the hand-off).
5. **Advisory, not gating** (B2b-3a). The verdict is REPORTED in the hand-off; `implement` always commits +
   hands off + exits 0. The review→tweak LOOP (re-prompt on REJECT/verify-FAIL) is **B2b-3b**.
6. **`[review]` config** (mirror `[verify]`): `workflow = "review-diff"` (+ optional output bound). Absent →
   review skipped. Reuses the workflow machinery; the review workflow is config+prompts, no new executor.

## Architecture

### `review-diff` workflow (config + prompts; agents run in the clone)
```toml
[[workflows]]
id = "review-diff"
[[workflows.nodes]]
id = "acceptance"
agent = "codex"            # cheaper model (owner); per-node configurable
prompt_file = "../prompts/review-diff-acceptance.md"
inputs = []
[[workflows.nodes]]
id = "correctness"
agent = "codex"
prompt_file = "../prompts/review-diff-correctness.md"
inputs = []
[[workflows.nodes]]
id = "design"
agent = "claude"
prompt_file = "../prompts/review-diff-design.md"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "../prompts/review-diff-synth.md"
inputs = ["acceptance", "correctness", "design"]
```
Each lens prompt: the READ-ONLY + BOUNDED contract (reuse the existing review-prompt contract — read/grep/
`git diff·log·show`, no writes/builds/network), the `{{input}}` (TASK + the base ref → "review `git diff
<base>..HEAD`"), and the lens's dimension. The **synth** prompt merges the three lenses (`{{acceptance}}`,
`{{correctness}}`, `{{design}}`), and ends with the prioritized findings + a final
`VERDICT: APPROVE|REJECT` line (APPROVE only if it delivers the task AND is correct/sound).

### `[review]` config (bin/a2a-bridge/src/config.rs)
`ReviewToml { workflow: String, #[serde(default)] max_output_bytes: Option<usize> }` →
`ReviewConfig { workflow: WorkflowId, max_output_bytes: usize }` (validate the workflow id; default the
bound). `RegistryConfig.review: Option<ReviewToml>` (`#[serde(default)]`).

### Pure `review.rs` (bin/a2a-bridge/src/review.rs — mirrors verify.rs)
```rust
pub enum Verdict { Approve, Reject, Undetermined }
pub enum ReviewOutcome { Ran { verdict: Verdict, summary: String }, NotConfigured, ConfigError, Failed }
/// PURE. Parse the synth output's `VERDICT: APPROVE|REJECT` line (last match wins; case-insensitive);
/// no match → Undetermined.
pub fn parse_verdict(synth_output: &str) -> Verdict;
/// PURE. Clamp captured review output (head+tail, like verify::truncate_output).
pub fn truncate_output(s: &str, max: usize) -> String;
/// PURE. The hand-off suffix: "review: APPROVE|REJECT|undetermined  (<short rationale>)" / "not configured".
pub fn outcome_suffix(o: &ReviewOutcome) -> String;
```
The pure parts are unit-tested; the workflow RUN (impure) is live-gated.

### Integration (implement_cmd `Action::Commit` arm, AFTER verify)
- Parse `review_cfg = cfg.review.as_ref().map(|t| t.to_config())` BEFORE `into_snapshot` moves cfg (same as
  verify_cfg). Build the review graph from `wf_map` (loaded before the snapshot move).
- After the commit + verify, if `[review]` is set: generate `git diff <base_sha>..HEAD` (via
  `implement::run_git` in the clone — for the prompt pointer; the lenses re-run it with tools), format the
  workflow input = `TASK:\n<task>\n\nReview the committed change (git diff <base_sha>..HEAD) in this repo —
  does it deliver the task? <bounded inline diff as a starting pointer>`, run the review workflow with
  `session_cwd = clone`, **capture the synth terminal output** (fix the discarded-`Terminal.output` seam in
  `implement_cmd`'s run loop), `review::parse_verdict` → append `review::outcome_suffix` to the hand-off.
- Reuses the same executor/registry already built; the `:ro` review agents are reaped by the shipped reaper
  (the end-sweep guard + the per-backend reap).

## Component / file boundaries

| Concern | Home |
|---|---|
| `review-diff` workflow + 4 prompts (acceptance/correctness/design/synth) | `examples/a2a-bridge.containerized.toml` + `prompts/review-diff-*.md` |
| `ReviewToml`/`ReviewConfig` + `RegistryConfig.review` | `bin/a2a-bridge/src/config.rs` |
| pure `Verdict`/`ReviewOutcome`/`parse_verdict`/`truncate_output`/`outcome_suffix` | `bin/a2a-bridge/src/review.rs` (new) |
| diff gen + review run + synth-output capture + verdict in hand-off | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) |

## Testing
- **Unit (no Docker):** `parse_verdict` (APPROVE / REJECT / case-insensitive / last-wins / no-line→Undetermined / a finding mentioning "APPROVE" doesn't false-match a non-`VERDICT:` line); `truncate_output` head+tail; `outcome_suffix` for each `ReviewOutcome`; `ReviewToml::to_config` (present/absent, bad workflow id). The implement-arm wiring is the impure orchestration (live-gated), with the verdict classification pulled into the pure `parse_verdict`/`outcome_suffix` (the coverage keystone, like verify's `outcome_suffix`).
- **Live gate (Docker, dogfooded on this repo):** `implement` a small change → the hand-off shows
  `verify: PASS …` + `review: APPROVE  (…)`; introduce a task the change does NOT satisfy (or a buggy
  change) → `review: REJECT  (…)`; assert the `:ro` review containers are reaped (the reaper); the commit +
  hand-off always happen (advisory).

## Deferred
- **B2b-3b:** the review→tweak loop (re-prompt an edit turn on the persistent clone on REJECT or verify-FAIL,
  re-commit/verify/review, bounded) + (optional) gating.
- Richer review code-nav tooling (tree-sitter / prism / LSP / symbol search) in the image + exposed to the
  `:ro` lenses (owner: materially improves review quality).
- Routing acceptance back to the **originator/dispatcher** (for orchestrated `implement`, not human-CLI).
- A spec-FILE input (beyond the task string) as the acceptance reference.

## Firewall
Designed from the bridge's own seams (the review workflow machinery, `implement_cmd`, the `[verify]`
pattern). **Dual review = containerized dogfood (now safe — the `:ro` reaper reaps the review lenses) +
a2a-local `codex-review` (gpt-5.5) backstop.** Dogfoods the bridge reviewing its own diffs.
