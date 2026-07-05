> NOTE -- codex-solo-rc-02: the runner's live judge returned a self-inconsistent grade twice (item_pass=True with false_findings>0) and was recorded as a judge_error. A single standalone re-judge (harness.judge, same judge/config/rubric/cached review) returned a self-consistent grade (item_pass=False, false_findings=1) which is folded here so this baseline is a complete 45/45. Flagged for the C2 spot-check: it is a clean item where the reviewer suggested adding tests and the judge counts that suggestion as one false finding.

# M3 review-quality eval report -- PROVISIONAL / n=15

> PROVISIONAL -- pending the owner's C2 spot-check. Fill `spotcheck.yaml`'s `agree:` fields, then re-run the eyeball pass described in evals/README.md.

- taskset: `review-seeded-v1`  n=15  cells: duo, codex-solo, claude-solo

Estimand: per-cell ABSOLUTE review quality (defect-level catch-rate + item-level pass/fail confusion) of the bridge's `code-review` workflow under three fixed configurations -- `duo` (the shipping shape, graded on the SYNTH output), `codex-solo` and `claude-solo` (single reviewer, graded RAW -- no synth pass). This is NOT a controlled ablation: cells differ simultaneously in (a) agent composition, (b) presence/absence of a synth pass, AND (c) review LENS -- `codex-solo` runs the correctness lens (`review-correctness`) while `claude-solo` runs the architecture lens (`review-architecture`, whose prompt targets 'what a correctness-only pass would miss'). Each solo cell mirrors that agent's role in the `duo` shape (the design is deliberate), but the consequence must be read explicitly: because this corpus is correctness-heavy (seeded defects are mostly correctness bugs), the architecture lens is STRUCTURALLY disadvantaged at seeded-defect recall by its lens, not by agent quality. `claude-solo`'s seeded-defect recall is therefore confounded with the architecture-lens/correctness-corpus mismatch and must NOT be read as 'claude is a worse reviewer'. Cross-cell deltas below are a descriptive paired-flips display, never a pass/fail criterion on their own.

## Per-cell results

Defect recall is the PRIMARY metric; both it and item-pass carry a Wilson 95% CI (per the spec's 'Wilson CIs throughout'). The CIs are descriptive only -- with 1-2 defects per seeded item and n=15 the intervals are wide and cluster-correlated; do not read a non-overlap as a significance test.

| cell | n | item-pass (95% CI) | defect recall (95% CI) | false findings (clean) | judge errors | calls skipped |
|---|---|---|---|---|---|---|
| duo | 15 | 11/15 = 0.733  (95% CI 0.480-0.891) | 11/11 = 1.000  (95% CI 0.741-1.000) | 23 (of 32 total) | 0 | 0 |
| codex-solo | 15 | 13/15 = 0.867  (95% CI 0.621-0.963) | 11/11 = 1.000  (95% CI 0.741-1.000) | 2 (of 15 total) | 0 | 0 |
| claude-solo | 15 | 11/15 = 0.733  (95% CI 0.480-0.891) | 11/11 = 1.000  (95% CI 0.741-1.000) | 17 (of 29 total) | 0 | 0 |

## Confusion matrix (TP/FP/TN/FN, base rate)

| cell | TP | FP | TN | FN | base rate | neutral matched |
|---|---|---|---|---|---|---|
| duo | 11 | 4 | 0 | 0 | 0.733 | 4 |
| codex-solo | 11 | 2 | 2 | 0 | 0.733 | 0 |
| claude-solo | 11 | 4 | 0 | 0 | 0.733 | 4 |

## Paired flip table (descriptive display, NOT a pass/fail criterion)

- **duo vs codex-solo**: both_pass=11 both_fail=2 only_duo=0 only_codex-solo=2  McNemar exact p=0.500
- **duo vs claude-solo**: both_pass=11 both_fail=4 only_duo=0 only_claude-solo=0  McNemar exact p=1.000

McNemar's p is REPORTED above for reference only -- per the M3 spec it is demoted to a paired-flips display and is never treated as a pass/fail gate, and no winner language is used here unless effects are huge and the paired flips are obvious.

## Excluded rows (judge errors + skipped calls)

- total judge_error rows across all cells: 0
  (excluded from every rate/confusion figure above -- a judge_error is a harness/judge failure, never silently folded into a result.)
- total skipped calls (reviewer call failed/empty, judge never invoked): 0
  (also excluded from every rate/confusion/recall figure -- a reviewer crash/timeout/empty output is a REVIEWER failure, scored as neither a pass nor a fail, so it can neither inflate recall nor launder into a clean true-negative.)

---
_Not aggregated across tasksets or repo states. See `run_manifest.json` in this results dir for the bridge git SHA, cell configs, and taskset id this report was rendered against._
