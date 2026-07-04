# M3 — Review-Quality Eval Harness (spec, v2)

**Status:** APPROVED for implementation — v1 reviewed by codex gpt-5.5 xhigh; all findings folded (absolute cwd + --out-is-a-file MAJORs, solo-raw estimand, claude-synth family fact, gemini config, catchability gate, PROVISIONAL stats framing, 11th corpus class).
**Source:** strategic-analysis M3 ("the strategic bet — self-hosted review quality — is
unmeasured"), pulled forward by the identity ruling. Prior art: the owner's
`~/code/prompts-skills-steering` harness (4 live-validated experiments) and its
`eval_framework_v1-1.md`; a recon digest ruled: **adopt the eval science, rebuild the
engineering bridge-native.**
**Branch:** `feat/m3-eval-harness`.

## What is measured

The bridge's `code-review` workflow as a function `review(cell) → findings + verdict`,
where a **cell** = {workflow variant, agents, effort}. v1 matrix (3 cells; per review):
1. `duo` — the shipping code-review workflow (codex + claude reviewers → **claude**
   synth, the actual shipping shape) — grades the SYNTH output (the shipping terminal
   artifact)
2. `codex-solo` — codex reviewer only, graded RAW (no synth: `review-synth.md` expects
   two lens variables and unknown vars render verbatim — the raw reviewer terminal is
   the cleaner estimand)
3. `claude-solo` — claude reviewer only, graded RAW

**Metrics (per cell, from the steering stack):** PRIMARY = defect-level **catch-rate**
(recall anchored to ground-truth defect ids — never inflated by judge-hallucinated
matches; CIs are descriptive given 1–2 defects/item cluster). Secondary: item-level
confusion, false-finding count on clean items (FRAGILE at n=4 clean — descriptive
only). Wilson CIs throughout; McNemar exact p REPORTED but demoted to a paired-flips
display, never a pass/fail criterion. `report.md` carries a visible
**`PROVISIONAL / n=15`** banner and avoids winner language unless effects are huge
and paired flips obvious (per review).

## Layout (new top-level `evals/` — outside the cargo workspace, outside default CI)

```
evals/
  README.md                 # how to run; cost expectations; judge calibration notes
  pyproject.toml            # uv-managed; deps: pyyaml, deepeval (integrity gate only); extras: opik
  harness/
    config.py               # fail-fast experiment config: validate every path BEFORE spending a turn;
                            # per-cell cost projection + budget gate; SAME-FAMILY-JUDGE GUARD
    runner.py               # drives `a2a-bridge run-workflow` per (cell × item); small process pool;
                            # per-item timeout; writes calls/<cell>-<item>.json (output + timing)
    judge.py                # BLIND binary judge: sees ONLY rubric + truth.yaml + normalized findings
                            # (extract findings/verdict sections, strip all other prose); isolated
                            # scratch cwd; retry once then JudgeError (nonzero exit, never silent)
    normalize.py            # findings/verdict extraction from workflow output
    metrics.py              # PORTED from steering (stdlib-only): wilson_ci, mcnemar_p, confusion
                            # (item- and defect-level), paired_flips
    check_taskset.py        # PORTED schema validator (field completeness, seeded/clean exclusivity,
                            # diff shape, no orphan dirs)
    report.py               # metrics.json + report.md (estimand statement, per-cell tables, flags)
  rubrics/review_judge.md   # ADAPTED from steering: binary-only, per-defect acceptable_match /
                            # reject_if bars, neutral_findings neither credited nor penalized
  tasksets/review-seeded-v1/
    manifest.yaml           # {id, seeded} rows
    items/<id>/{context.md, diff.patch, truth.yaml, fixture/}   # fixture/ = tiny buildable Rust crate
  configs/
    eval-agents.toml        # bridge config for eval runs: reviewer agents + the JUDGE agent
  ci/test_integrity.py      # deepeval BaseMetric (non-LLM) artifact-shape gate over a results dir —
                            # runs OFFLINE, zero agent calls; the steering pattern verbatim
  results/                  # gitignored EXCEPT committed baseline reports (results/baselines/)
```

## Taskset (Stage 0 — the 70%)

**Schema:** steering's, unchanged: `context.md` (≤25 lines), `diff.patch` (unified
diff), `truth.yaml` with per-defect `id / defect_class / location / hunk_lines /
description / root_cause / bad_behavior / minimal_trigger / acceptable_match /
reject_if`, plus `neutral_findings`; clean items carry `clean_rationale` +
`tempting_non_defects`. `fixture/` additionally holds the tiny Rust crate with the
defect APPLIED (the bridge's reviews are read-only-tools + `--session-cwd`, unlike
steering's tool-free inline-only reviews — items provide BOTH the inline diff and the
navigable fixture).

**v1 corpus: 15 items — 11 seeded (1–2 defects each), 4 clean.** Defect classes drawn
from this repo's own bug history (each class has a real ancestor):
lock-across-await (the W1-B class), config-validation bypass (the S6 nested-volumes
class), wire-leak redaction miss (the `{e}`-to-client class), error-swallowed
(`let _ =` on a fallible cleanup), TOCTOU check-then-act, off-by-one on a cursor/
pagination bound, unbounded channel/collection growth, cancellation-unsafety (partial
write on future drop), integer-truncation cast, stale-lock/double-release, and
**wrong-cwd/path propagation** (the 11th class, added per review — this repo's
recurring scar). Clean items include `tempting_non_defects` (e.g., an intentional
`biased;` select, a deliberate clone).
**Fairness gate for the risky classes** (cancellation, stale-lock, unbounded growth):
the diff must EXPOSE the invariant so a read-only reviewer can catch it without
executing tests — each seeded item carries a one-line "catchable-from-diff because…"
field in truth.yaml (`catchability`), human-checked at authoring.

**Authoring discipline:** every seeded item's fixture must `cargo build -j 1` (defects
are semantic, not syntax errors); every `acceptable_match` must be satisfiable by a
paraphrase and every `reject_if` must exclude the tempting near-miss; the
`catchability` line present on every seeded item; `check_taskset.py` green is the
merge gate for corpus commits. Generated task-specs embed `context.md` + the fenced
`diff.patch` in the body (task-spec `code-review` requires only Description +
Acceptance Criteria); **truth.yaml is NEVER exposed to executors** (keep it out of the
fixture/ dir so `--session-cwd` navigation cannot leak it).

## Judge

- **Cross-family, via the bridge itself:** the judge is a single-node workflow
  (`inputs=[]`, judge prompt = rubric + truth + normalized findings) run through
  `a2a-bridge run-workflow` against a **kiro** agent (v2.1 pivot — gemini's OAuth is
  dead on this machine, probe-verified `throwIneligibleOrProjectIdError`; kiro probe:
  "Logged in with GitHub"). Kiro = Amazon family — cross-lineage from EVERY agent that
  authors graded artifacts (codex + claude reviewers AND the claude synth — the guard
  covers the whole authorship set, not just "executors"). Copy the kiro entry shape
  from `examples/a2a-bridge.multi-agent.toml`; `evals/configs/eval-agents.toml` must
  pass `a2a-bridge validate --config`. "Tools-off" is enforced by: no
  `[[agents.mcp]]` on the judge entry (the ACP client already advertises no
  FS/terminal capability and rejects reverse FS/terminal calls) + a no-tools judge
  prompt. Fallback chain: **ollama-cloud open-weight model** (`kind="api"`,
  OLLAMA_API_KEY is set on this machine) → ollama-local `qwen2.5-coder:7b` (emergency
  only — a 7B is weak for rubric-following; if used, the report carries a judge-tier
  flag). Gemini re-enters the chain if its auth is restored.
  `config.py` owns a **harness-side family map** ({codex: openai, claude: anthropic,
  gemini: google, kiro: amazon}) since AgentEntry has no family field; it refuses a
  judge whose family appears in the graded-artifact authorship set unless explicitly
  overridden.
- Output contract: strict JSON (`{item_pass, defects: [{id, found}], false_findings}`)
  — parse-or-retry-once-or-JudgeError.
- **C2 calibration:** `spotcheck.yaml` — a random 10-item judge-decision sample
  rendered for human review, committed with the baseline report (the steering
  spotcheck pattern; the human pass is the owner's, not automated).

## Runner

Drives the REAL product path: `a2a-bridge run-workflow <cell-workflow> --input
<generated task-spec> --session-cwd <ABSOLUTE fixture path> --config
evals/configs/eval-agents.toml --out calls/<cell>-<item>.md` — per review MAJORs:
`--session-cwd` MUST be absolute (`SessionCwd::parse` rejects relative), and `--out`
is a FILE (the terminal node's output), not a directory; the runner wraps
timing/exit-code metadata in a sibling `calls/<cell>-<item>.json`. Note: local
`run-workflow` does not enforce `allowed_cwd_root` (the served path does) — fine for
local evals; documented in README for any future `--serve` runner. Task-spec generated per item from
`context.md` + `diff.patch` using the shipped `code-review` task-type conventions.
Concurrency 2 (subscription-quota politeness); per-item wall timeout; `--smoke` flag =
3 items × 1 cell; budget gate aborts before the matrix if projected turns exceed the
configured cap. Every run directory stamps: bridge version (`a2a-bridge --help` head /
git SHA), cell configs, taskset id — the C1 regression discipline.

## Tool usage (per the owner's directive, with the recon's honest reading)

- **deepeval** — the integrity CI gate (`ci/test_integrity.py`, custom non-LLM
  `BaseMetric` over run artifacts; pytest vocabulary). This is exactly how steering
  uses it in anger. NO deepeval LLM metrics (they'd need API keys the machine doesn't
  use, and the blind-judge architecture is strictly better calibrated here).
- **opik** — optional tracing hook ONLY (steering's pattern: lazy import inside
  try/except, enabled only when `OPIK_API_KEY` is set, silent fallback to
  `trace.jsonl`); NOT on the v1 critical path — if it costs more than the trivial
  hook, defer it (per review).
- **promptfoo — deliberately NOT used**, with reasoning recorded here: its load-bearing
  role in steering is parallel raw-CLI orchestration; the bridge IS the orchestrator
  under test, so a thin process pool over `run-workflow` replaces it without a Node
  dependency. (Spec-review may challenge this.)

## Definition of done

1. `check_taskset.py` green over the 15-item corpus; every fixture builds
   (`cargo build -j 1` per fixture); every seeded item has its `catchability` line;
   `a2a-bridge validate --config evals/configs/eval-agents.toml` green.
2. `ci/test_integrity.py` green offline against a fabricated results dir (no agents).
3. **Live smoke:** `--smoke` (3 items, duo cell) end-to-end with real codex+claude+
   gemini-judge: findings extracted, judge JSON parses, metrics/report render.
4. **Baseline run:** the full 3-cell × 15-item matrix executed ONCE on this machine;
   `results/baselines/2026-07-<d>-review-seeded-v1/report.md` committed — the repo's
   first measured review catch-rate — plus `spotcheck.yaml` for the owner's C2 pass.
5. Whole-branch dual review (opus + codex xhigh); merge; push. Eval runs are NEVER in
   default CI (real tokens); the deepeval integrity test may run in CI offline.

## Risks

- **Corpus quality is the whole game** (Stage 0 = 70%): mitigated by the ported
  validator, the build-gate, and routing corpus authoring to the strongest implementor
  with the schema as a hard contract.
- Judge validity: cross-family + blind + binary + C2 spotcheck; the AXIOM warning
  (complex agentic judges underperform simple rubric judges) argues for exactly this
  minimal judge.
- Cost: 3 cells × 15 items ≈ 45 workflow runs + ~55 judge calls per full run —
  budget-gated, smoke-first, never scheduled.
- Gemini agent availability on this machine (memory says the adapter shipped; verify
  live before the smoke — fallback kiro).
