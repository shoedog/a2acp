# ADR-0022 — Review-the-Diff → APPROVE/REJECT (Containerized Agents, Slice B2b-3a)

**Date:** 2026-06-06
**Status:** Accepted

**Builds on:** B2b-1 (`implement`, ADR-0019), B2b-2 (verify, ADR-0020), the `:ro` reaper (ADR-0021 — its
prerequisite: the review spawns `:ro` reviewer agents, now reaped). B2b-3b (the review→tweak loop) follows.

---

## Context

After `implement` commits a change to a quarantine clone and B2b-2 verifies it builds+tests, the operator
still has to judge whether the change is *good* and *delivers the task* — before merging. B2b-3a adds a
multi-agent review of the committed diff that surfaces an **APPROVE/REJECT** verdict in the hand-off,
**advisory** (the operator/originator makes the final accept at merge; the auto-fix loop is B2b-3b).

## Decision

`implement` runs an `implement-review` workflow on the committed diff after verify, appends the verdict to
the hand-off, and the post-commit tail is made infallible so the hand-off always prints.

- **Topology B — two diverse reviewers, all dimensions folded, → synth.** `reviewer_codex` + `reviewer_claude`
  each review the diff for **acceptance** (does it DELIVER the task?), **correctness**, and **design**
  (leaning to their model strength), → a `synth` node merges + emits the verdict. Diversity caps at 2 models
  today; two broad diverse reviewers keep full diversity at 3 calls; acceptance is covered by both. (3-focused-
  lens / 1-reviewer variants are alternate workflow defs; **adaptive depth** by `git diff --stat` is deferred.)
- **Reviewers navigate the clone** (`session_cwd=clone`, read-only git/grep/read); the diff is not inlined —
  the bridge passes the task + `base_sha`/`head_sha` and the `git diff` instruction. Read surface is the
  clone (the `:ro` mount is the hard boundary). Richer code-nav tooling (tree-sitter/prism/LSP) deferred.
- **Acceptance is first-class** (scoped to the task; spec-file input deferred); the synth's verdict rule is
  deterministic: **REJECT if any BLOCKER, or acceptance unmet, or a correctness MAJOR that means it's broken;
  else APPROVE.** Final acceptance is the originator's (operator); routing to a dispatching orchestrator deferred.
- **Model tier is an AGENT-level property** (`WorkflowNodeToml` has no model field) — `[review]` only names a
  workflow id; a reviewer's model = its `[[agents]]` entry.
- **Machine-parseable, unspoofable verdict.** Synth ends with `VERDICT: APPROVE|REJECT` + `SUMMARY:`.
  `parse_verdict` is **tail-anchored**: exactly one `^VERDICT:` line that must be the footer (only an
  immediately-following `^SUMMARY:` + trailing blanks); conflicting/footer-not-at-tail/unknown-token →
  Inconclusive; **never infers APPROVE**. Prefix matching is **byte-wise** (ASCII keywords) so a multi-byte
  char in a finding can't panic the parse.
- **Advisory, BOUNDED, never blocks the hand-off.** The review is bounded by a timeout (`select!`-cancel-
  then-keep-draining so the executor runs its cancel cleanup); every failure maps to a named outcome
  (`NotConfigured`/`ConfigError`/`NotLoaded`/`Incomplete`); the post-commit tail is **infallible** —
  `clone_cwd` is precomputed pre-commit (reused by verify+review, removing a latent B2b-2 post-commit
  `SessionCwd::parse?`), the post-commit stage check is best-effort, and `parse_verdict` can't panic.
  **No `?` (and no panic) between the commit and the hand-off.**

## Components

| Concern | Home |
|---|---|
| pure `Verdict`/`ReviewOutcome`/`parse_verdict`/`reduce`/`build_review_input`/`outcome_suffix` | `bin/a2a-bridge/src/review.rs` (new) |
| `ReviewToml`/`ReviewConfig` (parsed `WorkflowId`, timeout, bound) + `RegistryConfig.review` | `bin/a2a-bridge/src/config.rs` |
| `implement-review` workflow + `review-implement.md` + `review-implement-synth.md` | embedded `INIT_*` + `examples/a2a-bridge.containerized.toml` + `prompts/` |
| precompute `clone_cwd`; infallible post-commit tail; bounded timed review + `drain_review` | `bin/a2a-bridge/src/main.rs` (`implement_cmd`) |

## Cross-check + dual-review folds

Cross-checked by the bridge's own firewalled clean-room `design` workflow (independent of the spec — it
converged on the spine + caught the model-is-agent-level fact). Dual spec-review + dual plan-review
(containerized dogfood — leak-safe post-reaper — + a2a-local codex) drove: the post-commit-infallible tail
(+ the latent B2b-2 `?`), the verdict thresholds, the **tail-anchored** parse + conflicting→Inconclusive +
never-infer-APPROVE, the bounded timeout (`select!`-cancel-keep-draining, not drop), the full outcome
taxonomy, parsing the FULL synth + dumping the body on non-APPROVE, the pure `reduce` extraction, and the
`WorkflowStream`/init-count mechanics.

## Validation

- Unit (Docker-free): `parse_verdict` matrix (tail-anchoring, conflict→Inconclusive, footer-not-at-tail,
  garbage→Inconclusive, **multi-byte/em-dash no-panic**, SUMMARY adjacency, never-infer-APPROVE);
  `reduce` (failed-reviewer count + terminal); `build_review_input`; `outcome_suffix` all arms;
  `ReviewToml::to_config` (default/explicit/malformed-id→ConfigError). Full clippy `-D warnings` clean.
- Live gate (Docker, dogfooded on this repo): **APPROVE path** — `implement` a task-satisfying change →
  committed + `verify: PASS` + `review: APPROVE (<summary>)` + `:ro` reviewers reaped (1s). **Bounded
  timeout** — `timeout_secs=1` → `review: incomplete` + the commit/hand-off STILL print (exit 0) + cancel→
  reap. REJECT / NotLoaded / degradation share the proven wiring and are unit-tested (a forced live REJECT
  is impractical — agents deliver).
- **Live-gate finding:** an em-dash in a finding line (`MAJOR — none.`) panicked the byte-slice prefix
  check → fixed to byte-wise compare; a panic also breaks the always-print invariant.
- Coverage (floors per ci.yml): new code is the `a2a-bridge` bin — **review.rs 99.39%**, config.rs 93% —
  workspace **89.50%** (floor 85); the floored library crates (bridge-core/acp/api/workflow) are untouched.

## Deferred

- **B2b-3b:** the review→tweak loop (re-prompt on REJECT/verify-FAIL, re-commit/verify/review, bounded) +
  optional gating — reuses `parse_verdict`.
- Adaptive review depth (`git diff --stat` → light/standard/thorough); richer code-nav tooling
  (tree-sitter/prism/LSP); routing acceptance to the originator/dispatcher; a spec-FILE acceptance input;
  a forced live-REJECT harness.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
