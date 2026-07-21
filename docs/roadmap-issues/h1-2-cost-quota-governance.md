# Cost & quota governance (enforce budgets on the cost data already captured)

**Roadmap:** H1-2 (★★★) · **Labels:** `kind:enhancement`, `area:coordinator`, `area:observability`, `area:cli`, `priority:p1`, `status:triage`
**Related:** M4 Slice 1 (turn_log cost/token columns + `bridge_turn_cost_total`/`bridge_turn_tokens_total` already shipped); reliability R3 cost-budget accounting.

## Problem
`turn_log` already records `tokens` and `cost` per turn and the cost/token counters already exist, but
**nothing enforces a budget**. A misconfigured `run-batch` or fan-out (bad concurrency) is an unbounded spend
— "a misconfigured batch can burn a day of quota." The measurement substrate exists; enforcement + surfacing
does not.

## Scope
### 1. Admission budgets
- [ ] Extend the batch semaphore (`crates/bridge-coordinator/batch.rs`, `[batch]`) and the fan-out path with
      per-run / per-time-window cost ceilings.
- [ ] Refuse or queue admission when projected spend exceeds headroom; re-check after hashing — mirror R3's
      "prospective budget headroom at admission and again after hashing" so the two accounting models agree.
- [ ] Config: hard-abort ceiling vs. soft warn threshold (`warm_usage_warn_fraction` is prior art for soft).

### 2. Visibility
- [ ] Surface per-node token/cost in `run-workflow --out` and `task get` (today evals gate on turn *count*
      because dollar usage isn't surfaced on that path).

## Design considerations
- Cost truth is provider-dependent and sometimes absent (R3 saw observed cost of zero; R3b added sticky
  non-finite/non-USD-cost handling). **Reuse that sticky, fail-closed cost serializer** — do not invent a
  second one; treat unknown cost conservatively (block or warn per policy), never as zero.
- Exposing task usage on A2A `tasks/get` is a wire-shape change deferred in M4 — decide whether cost surfacing
  rides that change or stays operator-side (metrics + journal) first.

## Value
Directly protects the operator from cost blowouts, and is high-leverage because it builds on already-captured
data rather than new plumbing. Composes with the reliability program's cost-bound concerns.
