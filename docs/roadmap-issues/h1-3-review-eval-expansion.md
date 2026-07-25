# Review-quality eval harness expansion (measure the core thesis)

**Roadmap:** H1-3 (★★★) · **Labels:** `kind:enhancement`, `area:evals`, `area:observability`, `priority:p1`, `status:triage`
**Related:** M4 Slice 1 built `turn_log` eval columns (`prompt_id`/`model`/`effort`) specifically for this; no consumer exists yet.

## Problem
Self-hosted multi-agent review is the product thesis, but it is almost unmeasured: `evals/` grades only the
`code-review` workflow, over a 15-item seeded-defect corpus, last run 2026-07-04, and it does **not** consume
the `turn_log` eval columns. Every prompt/model/effort/depth decision is therefore a guess, and there is no
regression signal when upstream agents drift — which is the entire premise of the reliability program.

## Scope
- [ ] Expand corpora beyond `review-seeded-v1`: add design-review and implement-outcome tasksets, plus a
      judge-quality eval (validate the kiro judge itself). Keep the blind cross-family judge + family-overlap guard.
- [ ] Wire the harness to `turn_log`: GROUP BY `prompt_id`/`model`/`effort` → catch-rate, precision, and
      false-finding rate per cell. Close the loop between the metrics investment and actual decisions.
- [ ] Make it schedulable (nightly/weekly against pinned agents) but never automatic-in-CI (token spend).
      This becomes the **quality canary** complementing the reliability *compatibility* canary.
- [ ] Extend the budget cap to dollar budgets once cost is surfaced (see H1-2); today `--cap` is turn-count.

## Design considerations
- Field note FN-1: read-only reviewer `cargo test` stalls at `_dyld_start` on macOS, so reviews are
  code-trace-verified not run-verified. Any eval that depends on the reviewer running tests needs the
  dedicated non-RO build/test step or a containerized-Linux verify.
- Keep the existing offline CI gates (deepeval integrity, metrics math, normalize, config guards) token-free.

## Value
The measurement layer that makes every downstream quality decision data-driven; without it, review quality is
invisible and drift undetectable.
