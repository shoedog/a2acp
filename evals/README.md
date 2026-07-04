# M3 review-quality eval harness

Measures the bridge's `code-review` workflow as a function
`review(cell) -> findings + verdict`, where a **cell** is
`{workflow variant, agents, effort}`. See
`docs/superpowers/specs/2026-07-04-m3-eval-harness.md` for the full spec this
implements (v2.1). This directory is bridge-NATIVE: it drives the real
`a2a-bridge` CLI (`run-workflow`) as its only transport, both for the
reviewer cells and for the blind judge. It lives outside the cargo
workspace and outside default CI -- eval runs spend real tokens and are
never scheduled automatically.

Corpus authoring (`evals/tasksets/`) is a separate concern from this harness;
see `evals/harness/check_taskset.py` for the schema gate.

## Setup

```sh
cd evals
uv sync
```

This creates `evals/.venv` from `evals/pyproject.toml` (deps: `pyyaml`,
`deepeval`, `pytest`; optional extra `opik` -- see Tooling below). All
commands in this README assume `cwd=evals/` unless stated otherwise; adjust
paths if you run them from the repo root instead (e.g.
`uv run --project evals ...`, or just `cd evals` first).

You also need a release build of the bridge itself:

```sh
cargo build --release -p a2a-bridge   # from the repo root
```

The harness defaults to `../target/release/a2a-bridge` relative to
`evals/harness/config.py`; override with `--bridge-bin` if you built
elsewhere.

## Offline gates (no agent calls, safe to run anytime)

```sh
uv run pytest ci/test_integrity.py -q      # deepeval integrity gate (fabricated results dir)
uv run pytest ci/test_metrics.py -q        # ported wilson_ci/mcnemar_p/confusion/paired_flips
uv run pytest ci/test_config.py -q         # cross-family judge guard + budget gate
uv run pytest ci -q                        # all three at once

uv run python harness/check_taskset.py --help                       # arg parsing only
uv run python harness/check_taskset.py ../evals/tasksets/review-seeded-v1
uv run python harness/check_taskset.py ../evals/tasksets/review-seeded-v1 --build  # + cargo build -j 1 per fixture (slow)

../target/release/a2a-bridge validate --config configs/eval-agents.toml
```

`check_taskset.py` exits `2` (not `1`) if the taskset directory or its
`manifest.yaml` doesn't exist yet -- distinct from "checked it and it's
broken" (`1`).

None of the above spends a token or spawns an agent. The `deepeval` gate in
`ci/test_integrity.py` fabricates its own tiny `calls/`+`judge/` rows in a
`tmp_path`, then calls the REAL `harness.report.render()` on them -- it
exercises the real report/metrics code, not a second hand-written shape
description.

## Running the eval matrix (spends real tokens)

```sh
# Cheapest possible live check: 3 items, duo cell only.
uv run python -m harness.runner --smoke

# See the planned work + budget projection without touching any agent.
uv run python -m harness.runner --dry-run

# Full v1 matrix: 3 cells x 15 items = 45 workflow runs + ~45 judge calls.
uv run python -m harness.runner --out ../evals/results/2026-07-XX-review-seeded-v1
```

`runner.py` is the single pipeline entry point: it drives
`a2a-bridge run-workflow` per (cell x item) at concurrency 2 (subscription-
quota politeness -- do not raise this in production runs), grades each
completed call through the blind `judge` workflow, and renders
`report.md`/`metrics.json`/`spotcheck.{md,yaml}` at the end. A **budget
gate** (`--cap`, default 120 projected turns) aborts BEFORE spending
anything if the planned (cell x item) x (workflow + judge) turn count
exceeds the cap -- this is a turn-COUNT gate, not a dollar gate:
`a2a-bridge run-workflow` does not currently surface per-node token/cost
usage on its CLI output, so there is no live spend figure to gate on here.

### Cost expectations

Per the spec: 3 cells x 15 items ~= 45 `run-workflow` invocations (`duo`
spends 3 agent turns per invocation: 2 reviewers + 1 synth; the two solo
cells spend 1 turn each) + ~45-55 judge calls (kiro, one per graded item).
Budget for real subscription usage on all of codex/claude/kiro, not API
dollars. `--smoke` is the cheap sanity check (3 items, `duo` only: 3
`run-workflow` calls, 3 turns each, + 3 judge calls) -- run it first.

### Local `run-workflow` vs `--serve`

The runner always uses LOCAL `a2a-bridge run-workflow` (not `--serve`).
Local `run-workflow` does **not** enforce `allowed_cwd_root` the way the
served HTTP path does -- fine for local evals (the harness itself controls
every `--session-cwd` it passes, always an item's own `fixture/` dir,
resolved absolute and asserted to exist before the call). If a future runner
variant drives eval cells through a running `serve` instance instead, it
will need to either widen `allowed_cwd_root` to cover the taskset's fixture
paths or accept that gate's rejection for out-of-root fixtures -- this
harness's local-mode runner has no such restriction today.

`--session-cwd` MUST be an absolute path (`SessionCwd::parse` rejects a
relative one outright) -- `harness/runner.py`'s `run_workflow()` resolves
and asserts this before every call.

## The 3-cell matrix

| cell | workflow id | agents | graded artifact |
|---|---|---|---|
| `duo` | `code-review-duo` | codex + claude reviewers -> claude synth | the SYNTH output (shipping shape) |
| `codex-solo` | `code-review-codex-solo` | codex reviewer only | the raw reviewer output (no synth) |
| `claude-solo` | `code-review-claude-solo` | claude reviewer only | the raw reviewer output (no synth) |

Solo cells are graded RAW rather than through `review-synth.md` because that
prompt expects two lens variables (`{{correctness}}`/`{{architecture}}`) --
an unset one would render an empty string, not represent "no synth", so the
raw reviewer terminal output is the cleaner estimand for those two cells.

**Confound to read carefully (do NOT skip):** the cells differ on THREE axes
at once -- agent composition, presence/absence of a synth pass, AND review
**lens**. `codex-solo` runs the correctness lens (`review-correctness`);
`claude-solo` runs the architecture lens (`review-architecture`, whose prompt
explicitly targets "what a correctness-only pass would miss"). Each solo cell
mirrors that agent's role in the shipping `duo` shape, so the design is
deliberate -- but because this corpus is correctness-heavy (its seeded
defects are mostly correctness bugs), the architecture lens is structurally
disadvantaged at seeded-defect recall by its LENS, not by agent quality.
**`claude-solo`'s seeded-defect recall must NOT be read as "claude is a worse
reviewer"** -- it is confounded with the architecture-lens/correctness-corpus
mismatch. `report.md`'s estimand statement carries this same disclosure so a
reader of the committed baseline can't miss it.

## The blind judge

The judge is itself just another workflow: a single **kiro** node
(`evals/configs/eval-agents.toml`'s `judge` workflow), invoked through
`a2a-bridge run-workflow judge` exactly like the review cells. Kiro (Amazon
family) is cross-lineage from every agent that authors a graded artifact
(codex + claude reviewers AND the claude synth) -- `harness/config.py` owns
a harness-side agent-id -> model-family map and refuses to run a judge whose
family overlaps a cell's authorship set, unless `--allow-same-family-judge`
is passed explicitly.

**Why kiro and not gemini:** the M3 spec's v1 (2026-07-04) picked gemini as
the judge; v2.1 pivoted to kiro after gemini's OAuth was found dead on this
machine (probe-verified `throwIneligibleOrProjectIdError`). If gemini's auth
is restored, it re-enters the family map (`AGENT_FAMILY["gemini"] =
"google"`) and could be swapped back in by pointing `JUDGE_AGENT_ID` /
`evals/configs/eval-agents.toml` at a `gemini` entry -- **not done in
v1; do not add a `gemini` agent entry to eval-agents.toml without first
re-verifying its OAuth live**, since a broken agent entry that still passes
`validate` (validate never probes PATH/auth, only parses config) is a
guaranteed live-run failure. A further fallback CHAIN is documented but not
wired as a live config entry: an ollama-cloud open-weight model (`kind =
"api"`, when `OLLAMA_API_KEY` is set) is the next-best cross-family judge;
ollama-local `qwen2.5-coder:7b` is an EMERGENCY-only last resort (a 7B model
is weak at rubric-following -- if ever used, flag the resulting report's
judge tier explicitly rather than silently treating it as equivalent to
kiro/gemini).

**Judge isolation:** every judge call gets a fresh, EMPTY `--session-cwd`
(`tempfile.mkdtemp`, removed after the call) -- see `harness/judge.py`. The
judge sees ONLY the rubric (`rubrics/review_judge.md`) + the rendered
`truth.yaml` + the normalized findings block (`harness/normalize.py`) --
never the raw diff, the reviewer's own prompt/tool trace, or which cell
produced the findings.

**Output contract:** strict JSON,
`{item_pass, defects: [{id, found}], false_findings, neutral_matched}`.
`harness/judge.py` parses strictly, retries once on any malformed/
inconsistent response, and raises `JudgeError` (surfaced as a
`judge_error: true` row, never silently dropped) on a second failure. A
`judge_error` is EXCLUDED from every rate/confusion metric and the overall
`runner.py` process exits nonzero if any occurred.

## Judge calibration (C2 spot-check)

Every `report.render()` call also writes `spotcheck.yaml` (+ human-readable
`spotcheck.md`): up to 10 judged items, stratified round-robin across cells,
with an `agree: null` slot per item. The report is **PROVISIONAL** until a
human (the owner) fills in `agree: true/false` for each sampled item by
reading `spotcheck.md` against the judge's own recorded verdict, then
records disagreement informally (no automated `check_spotcheck.py` gate
ships in v1 -- see the Deviations note in the M3 harness delivery report).
`report.md` always carries a visible `PROVISIONAL / n=<item count>` banner
and an explicit estimand statement; it deliberately avoids winner language
across cells unless effects are huge and the paired-flips display is
unambiguous.

## Tool usage

- **deepeval** -- the integrity CI gate only (`ci/test_integrity.py`, a
  custom non-LLM `BaseMetric`). No deepeval LLM metrics: they'd need API
  keys this machine doesn't use, and the blind-judge architecture above is
  strictly better calibrated for this harness's purpose anyway.
- **opik** -- optional tracing hook only (`harness/runner.py`'s `_trace`):
  lazy-imported only when `OPIK_API_KEY` is set, else falls back to a
  `trace.jsonl` line per call/judge event in the results dir. Install it via
  `uv sync --extra opik`. Not on the v1 critical path.
- **promptfoo -- deliberately NOT used.** Its load-bearing role in the prior
  art (`~/code/prompts-skills-steering`) was parallel raw-CLI orchestration
  across providers; here the bridge itself IS the orchestrator under test,
  so a thin Python process pool over `run-workflow` replaces it without a
  Node dependency.

## Layout

```
evals/
  README.md                 # this file
  pyproject.toml            # uv-managed
  harness/
    config.py                # cells, family map, budget gate, taskset loader
    runner.py                # pipeline entry point: run -> judge -> report
    judge.py                 # blind judge (invokes `a2a-bridge run-workflow judge`)
    normalize.py              # findings-block extraction / failure-marker detection
    metrics.py                # ported stdlib-only stats (wilson_ci, mcnemar_p, confusion, paired_flips)
    check_taskset.py          # corpus schema validator (own CLI, own exit codes)
    report.py                 # metrics.json + report.md + spotcheck.{md,yaml}
  rubrics/
    review_judge.md           # the judge's grading rubric (embedded into the judge prompt)
    judge-task.md             # the `judge` workflow's bare {{input}} passthrough prompt_file
  tasksets/review-seeded-v1/  # corpus (owned by a separate authoring track)
  configs/eval-agents.toml   # bridge config: codex/claude reviewers + kiro judge + the 4 workflows
  ci/
    test_integrity.py         # deepeval gate (offline, fabricated results dir)
    test_metrics.py           # ported known-value tests
    test_config.py            # family guard + budget gate tests
  results/                   # gitignored except results/baselines/
```
