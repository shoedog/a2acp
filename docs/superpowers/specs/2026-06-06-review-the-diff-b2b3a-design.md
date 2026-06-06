# B2b-3a — Review-the-Diff → APPROVE/REJECT — Design

**Date:** 2026-06-06
**Status:** Draft (rev3 — folds the dual spec-review: containerized dogfood PRIMARY + a2a-local codex
backstop, both needs-changes; spine confirmed sound). rev2 folded the firewalled clean-room `design`-
workflow cross-check + owner Topology-B decision.
**Builds on:** B2b-1 (`implement`, ADR-0019), B2b-2 (verify, ADR-0020), the `:ro` reaper (ADR-0021 — its
prerequisite). B2b-3b (the review→tweak loop) follows.

## Goal

After `implement` commits the agent's change (and B2b-2 verifies it), run a **multi-agent review of the
committed diff** and surface an **APPROVE/REJECT** verdict in the operator hand-off — **advisory** (the
operator/originator makes the final accept at merge). Flow:
`edit → commit → verify → review-the-diff → hand-off-with-verdict`.

## Decisions (owner + dual-review fold)

1. **Topology B — two diverse reviewers, all dimensions folded, → synth.** Both reviewers (codex + claude)
   review the diff for **acceptance** (does it DELIVER the task? gaps, missing requirements, cases the task
   implies), **correctness** (bugs/regressions/edge-cases), **design** (architecture fit), each leaning
   into its own model strength (codex→correctness, claude→architecture — **emergent** via a shared prompt,
   not mechanized). A synth node merges → verdict. Diversity caps at 2 models today (codex=gpt-5.5; claude=
   subscription); two broad diverse reviewers keep full diversity at 3 calls; acceptance covered by both.
   (3-focused-lens / 1-reviewer variants are alternate workflow defs; **adaptive depth** by `git diff
   --stat` is a deferred fast-follow.)
2. **Acceptance is first-class, scoped to the TASK.** Only `a.task` is passed (the spec-FILE input is
   deferred), so dimension #1 = "delivers the **task** (incl. requirements the task implies)" — not "/spec"
   until the spec-file input lands. Final acceptance is the **originator's** (operator hand-off + merge;
   routing to a dispatching orchestrator deferred).
3. **Model tier is AGENT-level, not a node knob** (`WorkflowNodeToml` = id/agent/prompt_file/inputs only).
   `[review]` only NAMES a workflow id; a reviewer's model = its `[[agents]]` entry. NOTE: the codex/claude
   agents also back `code-review`/`spec-review`, so retuning a reviewer's model later means mutating a
   shared agent or adding a new agent entry (don't silently retune every workflow).
4. **Reviewers navigate the CLONE with read-only tools** (`session_cwd = clone`; the reader image ships
   `git`+`ripgrep`). Read surface = the clone working tree + `.git` (for `diff`/`log`/`show`); the `:ro`
   container mount is the HARD boundary (no reads outside `session_cwd`); the prompt forbids writes/builds/
   network. The diff is NOT inlined — the bridge passes the task + `base_sha`/`head_sha` and the instruction
   to `git diff <base>..<head>`; reviewers navigate. Richer code-nav (tree-sitter/prism/LSP) deferred.
5. **Determinism of the inputs.** `base_sha` = the pre-run source base the existing `implement` step
   resolves (`rev-parse <base_ref|HEAD>` before the clone); `head_sha` = the bridge's host commit. Agent-
   created commits are out of scope — `head_guard` rejects an agent-advanced HEAD before `Action::Commit`.
6. **Machine-parseable verdict, tail-anchored + unspoofable.** Synth ends with EXACTLY (nothing after):
   `VERDICT: APPROVE` (or `REJECT`) then `SUMMARY: <one-line reason>`. `parse_verdict` reads the **footer
   from the tail** (the final non-empty lines): the verdict is the LAST line matching
   `^\s*VERDICT:\s*(APPROVE|REJECT)\b` (case-insensitive) *only if it is in the trailing footer*; **multiple
   distinct `VERDICT:` lines → Inconclusive** (a quoted/fenced `APPROVE` in the body can't override a real
   REJECT); anything else → Inconclusive. **NEVER infer APPROVE.** SUMMARY = the line *immediately
   following* the chosen `VERDICT:` line iff it matches `^\s*SUMMARY:`; else empty.
7. **Verdict thresholds (deterministic rule in the synth prompt):** REJECT if **any BLOCKER**, OR
   **acceptance unmet** (the change does not deliver the task — regardless of the finding's tag), OR a
   **correctness MAJOR that means the change is wrong/unsound**. Otherwise APPROVE (MINORs / style →
   APPROVE, noted in SUMMARY). The synth states this rule so two synth runs agree.
8. **Advisory, BOUNDED, never blocks the hand-off — and the post-commit tail is INFALLIBLE.** The verdict
   is REPORTED; `implement` always commits + hands off + exits 0. After `host_commit` there is **no `?`**:
   the post-commit `stage_state` check degrades to best-effort (log, don't abort); `clone_cwd: SessionCwd`
   is **precomputed before the commit** (reused by verify + review — removes the post-commit
   `SessionCwd::parse?` at main.rs:624, a latent B2b-2 abort); the review run is **bounded by a timeout**
   (a cancel token fired after `review.timeout`); every review failure maps to a named outcome (below). The
   B2b-3b loop reuses `parse_verdict` unchanged.

## Architecture

### `implement-review` workflow (config + 2 prompts; reviewers run in the clone)
```toml
[[workflows]]
id = "implement-review"
[[workflows.nodes]]
id = "reviewer_codex"      # codex; leans correctness/blockers (emergent); covers accept+correct+design
agent = "codex"
prompt_file = "prompts/review-implement.md"      # NEW, folded 3-dimension, diff-native (shared by both)
inputs = []
[[workflows.nodes]]
id = "reviewer_claude"     # claude; leans architecture/acceptance (emergent); same shared prompt
agent = "claude"
prompt_file = "prompts/review-implement.md"
inputs = []
[[workflows.nodes]]
id = "synth"               # merge + emit the VERDICT/SUMMARY footer (the single terminal sink)
agent = "claude"           # synthesis is claude's strength (a synth failure → Incomplete, never blocks)
prompt_file = "prompts/review-implement-synth.md"   # NEW; {{reviewer_codex}} {{reviewer_claude}} {{input}}
inputs = ["reviewer_codex", "reviewer_claude"]
```
- `review-implement.md` (NEW, shared): READ-ONLY+BOUNDED contract (read/grep/`git diff`·`log`·`show` in
  `session_cwd`; no writes/builds/network; no reads outside the clone), the `{{input}}` (TASK + base/head
  SHAs + "review `git diff <base>..<head>`, navigate the repo"), the 3 dimensions, "you are ONE of two
  independent reviewers — cover all three, lean into YOUR model's strength; tag findings BLOCKER/MAJOR/MINOR."
- `review-implement-synth.md` (NEW): merge `{{reviewer_codex}}`+`{{reviewer_claude}}` (de-dup, resolve
  disagreements, note a missing reviewer if a leg failed), prioritized findings, the **verdict-threshold
  rule** (Decision 7), then the strict tail footer (Decision 6).
- Registered in BOTH the embedded defaults (`INIT_WORKFLOWS`/`EMBEDDED_PROMPTS`, like `code-review`; `init`
  emits it when codex+claude are selected) AND `examples/a2a-bridge.containerized.toml` (which **ships
  `[review] workflow="implement-review"`** so the dogfood path has it). Embedded prompt paths are top-level
  `prompts/...`; examples use `../prompts/...` — both coexist.

### `[review]` config (bin/a2a-bridge/src/config.rs, beside `VerifyToml`)
```rust
pub struct ReviewToml { #[serde(default="default_review_workflow")] workflow: String,    // "implement-review"
                        #[serde(default)] max_output_bytes: Option<usize>,               // absent/0 → 16*1024
                        #[serde(default)] timeout_secs: Option<u64> }                     // absent → e.g. 300
pub struct ReviewConfig { pub workflow: bridge_core::ids::WorkflowId,                     // PARSED pre-commit
                          pub max_output_bytes: usize, pub timeout: Duration }
impl ReviewToml { pub fn to_config(&self) -> Result<ReviewConfig, ConfigError>           // parses the id →
    /* WorkflowId::parse(workflow)? maps a malformed id to ConfigError BEFORE commit */ }
// RegistryConfig:  #[serde(default)] pub review: Option<ReviewToml>     // absent → step skipped
```
Net-new: `RegistryConfig.review` (only `verify` exists today) + `mod review` in main.rs.

### Pure `review.rs` (bin/a2a-bridge/src/review.rs — mirrors verify.rs)
```rust
pub enum Verdict { Approve, Reject, Inconclusive }
pub enum ReviewOutcome {
    Ran { verdict: Verdict, summary: String, reviewers_failed: usize }, // degradation in structured output
    NotConfigured,   // no [review]
    ConfigError,     // to_config Err (e.g. malformed workflow id) — captured pre-commit as Some(Err)
    NotLoaded,       // a VALID id absent from a successfully-loaded wf_map (typo) — soft, post-commit
    Incomplete,      // executor stream Err / missing terminal / timeout / cancel — the runtime catch-all
}
/// PURE. Tail-anchored footer parse (Decision 6): never infers Approve; conflicting verdict lines → Inconclusive.
pub fn parse_verdict(synth: &str) -> (Verdict, String);
/// PURE. {{input}} for the reviewers: task + base/head SHAs + the `git diff <base>..<head>` + navigate.
pub fn build_review_input(task: &str, base_sha: &str, head_sha: &str) -> String;
/// PURE. Hand-off suffix incl. degradation: "review: APPROVE  (<summary>)" / "review: REJECT (…)" /
/// "review: inconclusive (…)" + " [1 reviewer]" when reviewers_failed>0; "review: incomplete (timed out|
/// did not finish)" / "review: not configured" / "review: skipped (config error|unknown workflow)".
pub fn outcome_suffix(o: &ReviewOutcome) -> String;
```
Reuse `verify::truncate_output` for the stderr dump. The classification is the pure coverage keystone; the
executor RUN is impure (live-gated).

### Integration (implement_cmd, AFTER the verify suffix)
- BEFORE `into_snapshot`: capture `review_cfg = cfg.review.as_ref().map(|t| t.to_config())` (beside
  `verify_cfg`); keep `wf_map` (`.cloned()`). Precompute `clone_cwd: SessionCwd` (fallible — do it **here,
  pre-commit**) once; verify + review reuse it (no post-commit `SessionCwd::parse`).
- The post-commit tail is INFALLIBLE (no `?`): the existing post-commit `stage_state` check → best-effort
  (log on Err); verify uses the precomputed `clone_cwd`. Then the review stage:
  - `None → NotConfigured`; `Some(Err(_)) → ConfigError` (log); `Some(Ok(rcfg))` →
    `wf_map.get(&rcfg.workflow)`: `None → NotLoaded`; else run with a timeout token:
    `tokio::select! { run = drain(executor.run_with_context(graph.clone(), build_review_input(task, base_sha, sha), "impl-review-<id>", token.clone(), ctx_from_clone_cwd)) => …, _ = sleep(rcfg.timeout) => { token.cancel(); Incomplete } }`. Drain captures `Terminal.output` (the impl-edit loop discards it) AND counts `NodeFinished{ok:false}` reviewer legs; a stream `Err`/missing-terminal/non-Completed → `Incomplete`; Completed → `Ran { parse_verdict(synth), reviewers_failed }`.
  - Append `review::outcome_suffix(&outcome)`; `println!(handoff)`.
- Reuses the executor/registry; the `:ro` reviewers (agents already in `[[agents]]`) are reaped by the
  shipped reaper (`ro_sweep_targets` covers them; the one-shot end-sweep guard catches them on exit).

### Soft vs hard failure boundary
`load_workflows` already **fails loud pre-commit** (boot/early) on unknown agents, missing prompt files, and
bad DAGs. So a structurally-broken `implement-review` can never reach the advisory path. The ONLY soft,
post-commit cases are: `ConfigError` (malformed `[review]` id, from `to_config`), `NotLoaded` (a valid id
absent from a loaded `wf_map`), and `Incomplete` (runtime review failure/timeout). All degrade to a reported
suffix; the commit + hand-off always happen.

## Component / file boundaries

| Concern | Home |
|---|---|
| `implement-review` workflow + `review-implement.md` + `review-implement-synth.md` | embedded `INIT_WORKFLOWS`/`EMBEDDED_PROMPTS` (main.rs) + `examples/a2a-bridge.containerized.toml` + `prompts/` |
| `ReviewToml`/`ReviewConfig` (+ `WorkflowId` parse, timeout) + `RegistryConfig.review` | `bin/a2a-bridge/src/config.rs` |
| pure `Verdict`/`ReviewOutcome`/`parse_verdict`/`build_review_input`/`outcome_suffix` | `bin/a2a-bridge/src/review.rs` (new) |
| precompute clone_cwd pre-commit; infallible post-commit tail; timed review run + drain + suffix | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) |

## Testing
- **Unit (no Docker):** `parse_verdict` matrix — APPROVE/REJECT in the tail footer; missing→Inconclusive;
  TWO `VERDICT:` lines (a body-quoted APPROVE after a tail REJECT)→Inconclusive; footer-not-at-tail→
  Inconclusive; case-insensitive; `VERDICT: maybe`→Inconclusive; SUMMARY = immediately-following line only;
  a finding mentioning "approve" doesn't match. `build_review_input` asserts task + both SHAs + the diff
  instruction. `outcome_suffix` for every `ReviewOutcome` incl. `reviewers_failed>0` degradation.
  `ReviewToml::to_config` (absent/default/explicit/**malformed id→ConfigError**/timeout/max_output default).
- **Live gate (Docker, dogfooded):** `implement` a task-satisfying change → `verify: PASS …` +
  `review: APPROVE  (…)`; a task it does NOT satisfy (or a buggy one) → `review: REJECT …`; a `[review]`
  typo → `review: skipped (unknown workflow …)` + run returns Ok; kill a reviewer leg → suffix shows
  `[1 reviewer]`; the `:ro` reviewers reaped (poll-to-0); the commit + hand-off ALWAYS happen.

## Deferred
- **B2b-3b:** the review→tweak loop (re-prompt on REJECT/verify-FAIL, re-commit/verify/review, bounded) +
  optional gating — reuses `parse_verdict`.
- Adaptive review depth (`git diff --stat` → light/standard/thorough workflow); richer code-nav tooling
  (tree-sitter/prism/LSP); routing acceptance to the originator/dispatcher; a spec-FILE acceptance input;
  mechanized per-node lean prompts; decorrelating the synth sink from a reviewer model.

## Firewall
Designed from the bridge's own seams; cross-checked by the bridge's firewalled clean-room `design` workflow
(rev2) and reviewed by the containerized dogfood spec-review (leak-safe post-reaper) + a2a-local
`codex-review` (rev3). Dogfoods the bridge reviewing its own diffs.
