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

---

## Additional ideas — Fable pass

*Generated by a Fable-5 subagent (2026-07-17), independent of the first 10 and deliberately targeting angles
those under-explored: self-improvement loops, git-history/temporal search, human-in-the-loop, cross-repo A2A
delegation, and decision support. Numbered continuing from 10.*

### 11. Prompt self-tuning against the eval harness (`prompt-tune`)
**Pitch.** The bridge improves its own `prompts/` using its own eval score as the verify signal.
**Shape.** `LOOP` where the "code" under edit is a prompt file and the "test" is an eval run: an agent proposes an edit to e.g. `review-correctness.md` → `VERIFY` = run the external seeded-defect eval harness against the candidate prompt over N seeds → keep if catch-rate beats the baseline beyond a noise floor, else revert and re-prompt with the miss transcripts → bounded attempts → hand off a prompt diff + before/after scores.
**Value.** Turns the eval harness from a one-shot measurement into a closed improvement loop; every workflow using the tuned prompts inherits the gain. The Tier-1 "improvement journey" already happens by hand — this is that loop, automated.
**Seams.** `LOOP` (the `implement` clone/verify/commit machinery verbatim — it doesn't care the diff is a prompt), a custom verify profile shelling to the eval harness, `DETACHED` (eval sweeps are long).
**Maturity/risk.** Medium-high. The eval signal is noisy and expensive; the hard part is a statistically honest accept rule so the loop doesn't chase variance — plus reward-hacking (a prompt that games the judge, not the task).
**Open questions.** How many seeds per iteration buys an honest accept/reject at tolerable cost? Should the judging family be barred from authoring the edits it will score?

### 12. Verify-oracle git bisect (`bisect`)
**Pitch.** Find the commit that broke a behavior: an agent writes the repro once; `git bisect` + containerized verify do the search mechanically.
**Shape.** `PIPELINE` bracketing a mechanical search: agent turns "X broke since \<good-ref\>" into a deterministic repro script → `VERIFY` proves it red on head, green on good → the bridge drives `git bisect run <repro>` in the quarantine clone (no agent in the loop; one verify per step) → an agent explains the culprit (`git show`, `blame`, `log -L`) and optionally chains into `implement` with the culprit attached.
**Value.** Temporal root-causing `triage` (#4) can't do — triage reasons over the present tree; bisect searches history with ground-truth checks. Agent cost is O(1) while the search is O(log n) container runs.
**Seams.** `VERIFY` (per-commit hermetic build/test), the quarantine clone (bisect must never touch the live tree), the reviewers' git-archaeology toolset, `DETACHED` for long searches.
**Maturity/risk.** Medium. Non-building commits (`bisect skip` policy), flaky repros poisoning the search (run the oracle k times per step?), and per-step container churn are the hard parts.
**Open questions.** Does the repro-authoring turn get to iterate against `VERIFY` until red-on-head/green-on-good (a mini-loop → ADR-0024 two-engines shadow)? Bound by wall-clock or step count?

### 13. Task-spec elicitation interview (`intake`)
**Pitch.** Turn a vague ask into a validated typed task-spec by interrogating the human instead of guessing.
**Shape.** Interactive `PIPELINE`: agent reads the repo (read-only) + the raw ask → derives what the target task-type's schema needs that the ask doesn't pin down → emits its questions and parks → operator answers via `session inject` (queued into the next turn) → drafts the task-spec, runs `task-spec` validation → loops until it validates or rounds are exhausted → outputs a dispatch-ready `--input` file.
**Value.** Attacks the tax the orchestration notes name outright: spec-completeness burden and ambiguity-guessing are the biggest source of wasted runs. Every downstream workflow (`implement`, `spec-review`, `feature` #7) gets better inputs; the human answers five questions instead of writing a spec.
**Seams.** `session inject` (prepend/append + dedupe), the E7 typed task-spec schema + validator as the machine-checkable "done", `NAV` for repo-grounded questions, `DETACHED` (answers may arrive hours later).
**Maturity/risk.** Low-medium. Machinery is shipped; the craft is bounding the interview (a completeness rubric derived from the schema, max rounds) and not asking what the repo already answers.
**Open questions.** Where does "waiting on human" surface (task watch event? notification)? After a timeout, fail — or emit a spec with explicit ASSUMPTION blocks?

### 14. Cross-repo consumer-impact review (`contract-check`)
**Pitch.** When repo A changes a surface repo B consumes, a peer bridge *sitting on repo B* reviews the change from the consumer's side.
**Shape.** `FANOUT` with a remote arm: a local lens reviews the diff as producer (`SLICE` for the changed surface) ∥ `DELEGATE` the changed-API summary to an A2A peer whose bridge mounts the consumer repo — the peer runs its own advertised `consumer-impact` skill (its agents grep *its* call-sites, run *its* verify against the proposed surface) → local synth merges producer + consumer findings into one compatibility verdict.
**Value.** The one shape where delegation beats a local agent *in kind*: no single agent ever mounts both repos; each side keeps its own isolation, credentials, and verify profiles; the consumer assessment is grounded in real call-sites, not the producer's guess. Dogfood pairs exist today: bridge ↔ eval harness, bridge ↔ prism MCP contract.
**Seams.** `DELEGATE` (bridge-a2a-outbound), `agent_card` per-workflow skill discovery, `FANOUT`, `SLICE`, task-spec as the cross-repo interchange format.
**Maturity/risk.** Medium. The interchange contract is the hard part — what crosses the wire, and both bridges' verdict formats must agree (wants H2-3 structured output on both ends).
**Open questions.** Well-known peer skill id convention vs. reading the peer's agent card? When the peer is down, degrade to producer-only with an explicit UNVERIFIED-consumer marker?

### 15. Run-journal autopsy (`run-autopsy`)
**Pitch.** Mine the bridge's own durable task store and transcripts for wasted-run patterns, and turn them into prompt/config fixes.
**Shape.** `FANOUT` over digests of the last N runs (SQLite store + node telemetry + per-member usage): one lens mines waste (retries, tool-denial loops, ambiguity-guessing, Inconclusive verdicts), one mines contract violations (unstaged edits discarded, malformed `VERDICT:` footers, drain incompletes), one mines cost/latency outliers per node × agent × effort → synth emits a trend table + ranked concrete prompt/`[[agents]]`/depth-threshold edits, each citing offending transcripts.
**Value.** Ops/log analysis pointed at the bridge's own exhaust — observability with an *actionable* output (edit proposals, not dashboards). Distinct from `triage` (#4: a target repo's failure) and `prompt-tune` (#11: synthetic eval signal): this learns from production traces.
**Seams.** the durable store (ADR-0010/0011) as corpus, the panel's usage-telemetry plumbing, `FANOUT`, read-only; proposals chain into #11 for eval-verified adoption.
**Maturity/risk.** Low-medium. Needs a deterministic journal→digest pre-summarizer (transcripts won't fit in context raw); the lenses themselves are prompt work.
**Open questions.** Retention/PII policy for transcripts fed to third-party families? Scheduled weekly canary vs. on-demand after a bad day?

### 16. Decision-precedent gate (`precedent`)
**Pitch.** Before a new spec/design lands, check it against what the repo already *decided* — 32 ADRs and the git history — and surface collisions with a supersede path.
**Shape.** `PIPELINE`: extract the proposal's load-bearing claims → an archaeologist hunts precedent per claim (ADR corpus; `git log -S/-G` for when a constraint appeared; `blame`/`log -L` on the code embodying it) → classify each hit CONFIRMS / CONSTRAINS / CONTRADICTS → a brief: "ADR-0016 already rejected this; superseding requires X", with citations.
**Value.** Decision support the review lenses don't give: `spec-review` judges a spec on its own terms; this judges it against institutional memory. The forward-looking twin of doc-drift (#6) — proposal-vs-decided instead of doc-vs-code — targeting the observed failure of proposals silently re-litigating settled ADRs.
**Seams.** git archaeology (already in every reviewer's toolset), `NAV` to find the code embodying a decision, read-only; composes as a pre-stage of `spec-review`/`design` or standalone.
**Maturity/risk.** Low. No new machinery; the design work is the claim-extraction prompt and a one-line-per-ADR index that fits in context (itself checkable by #6).
**Open questions.** Gate (CONTRADICTS blocks until the proposal names the ADR it supersedes) or annotation only? ADR index maintained in-repo (drift risk) vs. rebuilt per run (cost)?

### 17. Supervised implement with live veto (`implement --supervised`)
**Pitch.** Keep `implement`'s warm loop, but give the human suspend/veto/steer points at the loop's own seams.
**Shape.** `LOOP` with human gates at the Rust-loop boundaries — never inside agent turns, respecting ADR-0024's two engines: after the edit commit (diff + verify shown), after each review verdict, before merge hand-off. Each gate emits a structured checkpoint and parks; the operator approves, aborts, or steers via `session inject` into the *same warm session* — the agent receiving the steer is the one that made the edit, context intact — then the loop resumes.
**Value.** The trust ramp between advisory review and full autonomy: high-stakes repos (external-review-gated merges) get an autonomous implementer that can be course-corrected mid-run without killing the container, the session, or the resume checkpoint. Distinct from #7's stage gates — those sit between whole workflows; these sit inside the tweak loop, where warm context is the asset preserved.
**Seams.** the warm session (ADR-0024), `session inject` + `session permit` (approve/deny/modify/escalate), resume checkpoints (ADR-0026 — a gate is a named durable suspend), merge hand-off (ADR-0027).
**Maturity/risk.** Medium. Machinery exists; the hard parts are gate semantics under absence (a 4-hour-unanswered gate: park durably and reap, or stay warm and burn idle?) and proving inject-then-resume respects the drain-turn contract.
**Open questions.** Gates as per-repo policy (`[implement] gates = ["pre-merge"]`) or per-invocation flags? Does an injected steer count against the bounded attempt cap?

### 18. Routing calibration from replayed history (`route-calibrate`)
**Pitch.** Learn which agent family × effort tier is *sufficient* per task-type by replaying real past inputs across the matrix and judging blind.
**Shape.** `DETACHED FANOUT`: sample past task-specs from the store, stratified by task-type → re-run each across the configured matrix (kiro / codex-medium / codex-xhigh / claude / cheap api) → a family-firewalled judge scores blind (reusing the self-judging guard) → aggregate with real per-run usage/cost telemetry into a routing table: task-type → cheapest agent+effort whose quality holds → emit reviewed `[[agents]]`/workflow-config suggestions.
**Value.** Cost engineering only the bridge can do: it already captures per-member usage (the panel's `{{workflow.costs}}` plumbing) and holds the replay corpus. Consensus (#2) spends *more* per answer at run time; this spends once offline to make every future run cheaper. Effort pinning (ADR-0029) provides the knobs; nothing today says how to set them.
**Maturity/risk.** Medium. Statistical honesty on small strata; replayed inputs referencing stale repo state need filtering; the sweep's own cost must be capped.
**Seams.** the store (replay corpus), `FANOUT` across `[[agents]]` entries, `judge` + family-overlap guard, cost telemetry, `DETACHED`.
**Open questions.** When does calibration go stale — re-run on live-catalog change, since model updates silently move the frontier? Advisory config text only, or anything auto-applied?

### 19. Repo intake brief + config bootstrap (`repo-brief`)
**Pitch.** Point the bridge at an unfamiliar repo; get back an onboarding brief *and* a working bridge config for that repo — the bridge onboarding itself.
**Shape.** `FANOUT` of grounded probes: cartographer (prism repo map + module deps → architecture sketch) ∥ ops (in a container, actually *attempt* build/test/lint until a verify profile provably runs) ∥ conventions/history (churn×recency hotspots, commit conventions via git archaeology) ∥ risk (untested hotspots) → synth two artifacts: a human brief where every claim carries its probe evidence, and a generated `a2a-bridge.toml` (verify profile + review workflows + `session_cwd`) that must pass `a2a-bridge validate` and a `smoke-*` run before it is reported.
**Value.** Knowledge extraction with a machine-checkable half: onboarding the bridge onto a project is today hand-written config (the onboarding doc's whole audience). One detached run replaces it — and the human brief is the by-product. Only possible *because* of containerized probing: the ops lens can safely try builds on unvetted code at the right sandbox tier.
**Seams.** `NAV`/prism repo map, `VERIFY` (Tier-2 probe of unvetted code, ADR-0032), the polyglot language profiles (ADR-0031) for per-language LSP wiring, `validate` + `smoke-*` as the acceptance gate, `DETACHED`.
**Maturity/risk.** Medium. Hallucinated build steps are the failure mode — mitigated by "no claim without a probe result" and the validate+smoke gate; monorepos and multi-language layouts stretch the verify-profile generator.
**Open questions.** How much container budget for the ops lens to find a green build (try cargo/npm/make in sequence)? Should the generated config land as a PR under the target repo's `tools/a2a-bridge/` per the onboarding layout convention?

### 20. Egress-audited evidence brief (`evidence`)
**Pitch.** For decisions needing *external* facts — adopt dependency X? is CVE-N reachable here? — gather web evidence inside egress-controlled containers and adversarially cross-verify before synthesizing.
**Shape.** `FANOUT` cross-family in open-egress containers: two researcher lenses (different families) independently gather cited sources (advisories, changelogs, issue trackers) ∥ a local `NAV` lens establishes repo-side facts (is the vulnerable API even called?) → a verifier lens re-fetches and checks each cited claim (dead link, misquote, version mismatch) → synth a decision memo separating verified externals, repo-grounded facts, and unverifiable claims.
**Value.** Decision support no shipped workflow gives — everything today is repo-internal. The egress-tier machinery makes web-touching agents *auditable* (which container talked to what) rather than forbidden; the differentiator over "ask a chatbot" is the cross-family verification pass plus repo grounding (a CVE memo that knows your call-sites).
**Seams.** the egress-controlled container path (ADR-0013 + the open-egress example config), `FANOUT` cross-family, `NAV` for the reachability half, the sandbox-tier policy (fetched web content is untrusted input — researcher outputs reach the synth as quarantined text).
**Maturity/risk.** Medium-high. Prompt-injection via fetched pages is the real risk — researcher lenses read hostile content in a tier that must never reach a repo write path; and citation verification must actually re-fetch, not vibe-check.
**Open questions.** Per-run egress allowlist (advisory domains only) vs. open? Should the repo-reachability lens run in a *separate* no-egress container so web-tainted context never co-resides with repo access?

### Cross-idea notes (Fable pass)
#11/#15/#18 form a self-improvement stack — production traces (store) → mined hypotheses → eval-verified prompt/routing changes — sharing the task store as corpus and the eval harness as arbiter; build the journal→digest extractor once for all three. #13/#17 are the two halves of the human-in-the-loop story on already-shipped machinery (`session inject`/`session permit`): intake before dispatch, veto during the loop — they should share one design for durable "waiting on human" state. #14 is the only idea that *requires* A2A and is worth building for that reason alone: it exercises delegation, agent-card skill discovery, and cross-bridge interchange, none of which any local workflow touches. The ADR-0024 two-engines constraint shadows #11 and #12's inner loops — like `implement`, they belong as Rust-coded loops, not workflow-DAG features. Cheapest first builds: #16 (precedent — read-only, pure prompt craft) and #13 (intake — machinery shipped; the schema *is* the rubric).
