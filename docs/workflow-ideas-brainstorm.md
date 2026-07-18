# Workflow ideas — brainstorm

**Created:** 2026-07-17
**Status:** exploratory brainstorm, non-committed. Seeds for new `[[workflows]]` (and a few implement-style
loops) that exploit the bridge's existing strengths: multi-agent cross-family DAGs (fan-out / pipeline /
fan-in), containerized verify, prism diff-slicing, LSP nav, durable detached runs, and A2A delegation.
Each idea lists the seams it reuses, a rough maturity/risk, and open questions — none are specs yet.

**Existing workflows for reference** (don't reinvent): `code-review`, `spec-review`, `plan-review`, `design`
(all draft→refine per-lens → synth), `implement` (clone→warm-edit→verify→review→tweak→merge), `panel`,
`judge`, and the `smoke-*` acceptance workflows. The ideas below are *new shapes*, not restatements.

**Primitive legend used below:** `FANOUT` parallel lenses → synth · `PIPELINE` staged nodes · `LOOP`
implement-style edit/verify iteration · `VERIFY` containerized build/test · `SLICE` prism diff-slice ·
`NAV` lsp-mcp · `DETACHED` durable resumable run · `DELEGATE` A2A peer.

---

## 1. Adversarial security review (`security-review`)
**Pitch.** A dedicated threat-modeling lens on a diff, separate from general `code-review`.
**Shape.** `FANOUT` three roles over a diff, then synth: (a) *attacker* — enumerate the changed attack surface
and, using `SLICE`, trace concrete taint paths from an untrusted source to a dangerous sink; (b) *defender* —
propose mitigations and check existing guards; (c) *scope* — classify each finding by exploitability and blast
radius. Synth emits a ranked, dedup'd findings report (later: SARIF via H2-3 structured output).
**Value.** The project already values security review — its own dogfooding caught the S6 sandbox hole and real
blockers in ADR-0030. A standing, structured red-team-your-own-diff workflow makes that repeatable instead of
ad hoc. Defensive use: reviewing changes you own before merge.
**Seams.** `FANOUT`, `SLICE` (taint paths), `VERIFY` optional for PoC-in-a-sandbox. Run adversarial-content
inputs at Tier 2.
**Maturity/risk.** Medium. Prompts are the hard part (attacker lens tends to hallucinate exploits — needs the
same WRONG/SMELL discipline as `code-review`, with a "name the constructible state" requirement).
**Open questions.** Should the attacker lens be allowed to *run* a PoC in a sandbox, or stay static? How to
keep false-positive rate low enough to be trusted as a gate vs. advisory only?

## 2. Cross-family consensus / tie-break (`consensus`)
**Pitch.** Run the same task through N agent families, measure agreement, escalate disagreement.
**Shape.** `FANOUT` the identical prompt to codex / claude / kiro; a cross-family `judge` scores agreement;
if they agree → return consensus; if they diverge → escalate to a higher-effort tie-breaker (or emit a
"needs human" flag via the H2-5 clarify channel).
**Value.** Cheap variance reduction for high-stakes single answers (a security verdict, a migration decision).
The eval harness already trusts cross-family judging; this productizes it for live use.
**Seams.** `FANOUT`, `judge` (reuse the family-overlap guard so a family never judges itself), structured
verdict schema (H2-3).
**Maturity/risk.** Low-medium. Mostly prompt + a small agreement metric.
**Open questions.** What's the agreement metric for free-text (semantic similarity? structured-field match?)
Does the tie-breaker get to see the divergent answers, or judge blind?

## 3. Test generation + mutation scoring (`test-gen`)
**Pitch.** Write tests for a diff, then prove they actually catch bugs.
**Shape.** `PIPELINE`: agent writes tests for the changed code → `VERIFY` runs them green → a mutation step
introduces seeded faults into the changed code and re-runs → report a catch-rate (mutation score) + the
surviving mutants.
**Value.** Directly attacks FN-1 (reviews are code-trace-verified, not run-verified) and gives `implement` a
principled "are these tests any good" signal instead of "they pass." Complements the eval harness's own
seeded-defect approach.
**Seams.** `VERIFY` (containerized, per-language profiles already exist), `LOOP` for the write→verify cycle.
**Maturity/risk.** Medium-high. Mutation tooling per language is the lift; could start with an off-the-shelf
mutant tool (e.g. `cargo-mutants`) wired into a verify profile.
**Open questions.** Generate mutants mechanically (tool) or via an agent? Cap mutation cost — full mutation
testing is expensive; sample the diff hunks?

## 4. Incident triage / root-cause (`triage`)
**Pitch.** Given a failing log/stacktrace + repo, produce a root cause with alternatives ruled out.
**Shape.** `PIPELINE` in a read-only container: agent forms ranked hypotheses → runs probes (grep, targeted
`VERIFY`, `NAV` to walk call paths) recording a hypothesis→probe→result log → synth settles a root cause and
*shows the evidence that ruled out the alternatives*.
**Value.** Mirrors the maintainer's own debugging discipline (hypothesis-probe-result; never fix on first
plausible cause) and the reliability program's incident RCAs — this workflow *is* that discipline, automated.
**Seams.** `PIPELINE`, `VERIFY` (read-only), `NAV`, durable log via the journal.
**Maturity/risk.** Medium. Guard against the classic failure: stopping at the first plausible cause. The synth
prompt must require an explicit ruled-out-alternatives section.
**Open questions.** How much probe autonomy (arbitrary shell in a sandbox vs. a fixed probe vocabulary)? When
does it hand off to a human vs. propose a fix (chain into `implement`)?

## 5. Dependency upgrade + auto-migration (`dep-upgrade`)
**Pitch.** Bump a dependency, and if it breaks, migrate the code to green.
**Shape.** `LOOP`: apply the version bump → `VERIFY` (build/test) → if red, an agent attempts the migration
edits → re-`VERIFY` → iterate to a bounded cap → hand off a PR-ready branch + a risk summary of what changed.
**Value.** Turns the dependabot/renovate signal (H2-6) into landed, verified upgrades instead of red PRs a
human has to fix. High-frequency, high-toil task.
**Seams.** `LOOP` + `VERIFY` (the whole `implement` machinery), Tier 3 write container, `merge` hand-off.
**Maturity/risk.** Medium. Mostly a specialized `implement` prompt + input shape (the dependency + target
version). Risk: silent semantic breakage that compiles+passes — pair with H1-3 test quality.
**Open questions.** Single dep or a batch sweep? How to bound "migration creep" (an agent rewriting unrelated
code)? Require the diff to touch only call-sites of the changed API?

## 6. Doc/code reconciliation (`doc-drift`)
**Pitch.** Find docs/ADRs whose claims contradict the code, and propose fixes.
**Shape.** `FANOUT` over doc sections: one agent extracts each doc's factual claims + a grep-able code anchor;
a checker verifies each claim against current code via `NAV`/grep; synth lists contradictions with proposed
edits. Optionally chain into `implement` to apply the doc edits.
**Value.** Attacks the *systemic* doc-drift cost this repo keeps paying (README vs. shipped Coordinator
migration; ADR-0031 vs. shipped MCP code; onboarding vs. ADR-0025). This is the automated form of roadmap
item H0-1's "hygiene sub-check."
**Seams.** `FANOUT`, `NAV`, read-only.
**Maturity/risk.** Low-medium. The claim→anchor extraction is the interesting prompt design.
**Open questions.** Run as a scheduled canary (like the compatibility canary) or on-demand pre-release? How to
avoid churn on intentionally-aspirational docs (roadmaps describe the future by design)?

## 7. End-to-end feature pipeline (`feature`)
**Pitch.** Spec doc in, reviewed branch out, as one durable run.
**Shape.** `DETACHED PIPELINE` chaining the *existing* workflows: `spec-review` → `plan-review` → `implement`
→ `code-review` → (optional) `merge`, with per-stage checkpoints and human-approval gates between stages.
**Value.** The flagship "autonomous feature" story, composed from already-shipped, already-reviewed pieces
rather than new orchestration. Showcases the durable/resume machinery end to end.
**Seams.** `DETACHED` (durable task store, resume), the full existing workflow set, `LOOP` inside `implement`.
**Maturity/risk.** Medium — mostly composition + gating, *but* the spec→plan→implement handoff and the
loop-inside-a-pipeline interaction bump into the two-engines constraint (ADR-0024); cleanest after H3-1
(engine unification) or orchestrated at the Coordinator level with explicit gates in between.
**Open questions.** Where do human approval gates sit (between every stage, or only before write/merge)? How
much context carries stage-to-stage vs. re-derived from artifacts?

## 8. Benchmark / performance-regression (`benchmark`)
**Pitch.** Compare a benchmark suite before/after a diff, flag regressions.
**Shape.** `PIPELINE`: `VERIFY`-style run of a benchmark suite on base → on the diff → statistical compare →
flag any metric beyond a threshold, with the offending change highlighted.
**Value.** The strategic analysis lists a benchmark suite as research-needed (cold vs warm start, handshake,
SQLite under batch, reattach fold, translator on large artifacts) — this workflow is the vehicle for it, and
becomes a perf-regression gate.
**Seams.** `VERIFY` (hermetic container for stable env), durable results for trend tracking.
**Maturity/risk.** Medium-high. Benchmark stability in containers is the hard part (noise, warmup, pinned CPU).
**Open questions.** Micro-benchmarks (criterion) vs. end-to-end bridge benchmarks? Store a baseline series
(like the eval baselines) for trend detection?

## 9. Migration / codemod sweep (`sweep`)
**Pitch.** Apply one mechanical transform across many files, verified per shard.
**Shape.** `FANOUT` shards (per file/module, each in its own worktree via the existing isolation) → each shard:
apply transform → `VERIFY` that shard → collect → merge. Fan-in reports which shards succeeded/failed.
**Value.** Large-scale refactors the codebase actually needs (god-file decomposition H3-4, an API rename,
`AgentBackend` trait split) become one supervised sweep instead of hand-editing dozens of sites.
**Seams.** `FANOUT`, per-shard `WorktreeBackend` isolation, `VERIFY`, `merge`.
**Maturity/risk.** Medium-high. **Blocked on same-target write locks (H2-1)** for safe concurrent writes to
one repo; until then, run shards serially or against disjoint paths.
**Open questions.** Deterministic transform (codemod script the agent writes once, applied mechanically) vs.
per-shard agent edits? How to merge shards that touch a shared file?

## 10. Release-readiness / changelog (`release-check`)
**Pitch.** Given a diff range, produce release notes + a go/no-go.
**Shape.** `PIPELINE`: agent drafts a `CHANGELOG` entry from the diff range → run the deterministic release
preflight (version match, gates, `compatibility.md` status) → synth a go/no-go with the compatibility matrix
state and any STALE/UNKNOWN rows called out.
**Value.** Automates the release-notes toil and turns the compatibility matrix + gates into a single
readiness verdict — ties into reliability R4 (promotion gate) without duplicating its machinery.
**Seams.** Read-only + deterministic gates; consumes `compatibility.md` and the release workflow's checks.
**Maturity/risk.** Low-medium. Deterministic parts already exist; the changelog draft is the agent part.
**Open questions.** Should it *open* the release PR/tag, or only produce the artifacts for a human? How does it
reconcile with R4's promotion gate so they don't disagree?

---

## Cross-idea notes
- **Structured output (H2-3) is a force multiplier** for #1, #2, #3, #8, #10 — several of these want
  machine-readable verdicts/scores, which the current char-cap coalescing can't cleanly provide.
- **A "quality canary" pairing:** #6 (doc-drift) and an eval-backed review canary (roadmap H1-3) both want to
  run *scheduled* against the repo/pinned agents — the same scheduling substrate the reliability program is
  building for compatibility canaries could host all three.
- **Two-engines constraint (ADR-0024)** shadows #4, #5, #7, #9 wherever they want a loop *inside* a pipeline;
  they're cleanest after engine unification (roadmap H3-1) or with explicit Coordinator-level gating.
- **Start where the seam is cheapest:** #6 (doc-drift) and #2 (consensus) are the lowest-risk first builds —
  read-only, mostly prompt + a small metric, no new isolation or write path.
