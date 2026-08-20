# A2a retry — mandatory sizing stop. The cap is wrong by ~3x on the evidence rows.

The retry refused to edit and staged nothing, citing the specification's own
mandatory pre-edit sizing stop:

> The accepted candidate already consumes 207/220 counted lines in
> `checked_scan.rs`'s test row. At least 11 required scenarios must remain
> child-private, needing a minimum 22 declaration lines, which reaches 229/220
> before assertions.

That is the stop working as designed, and its arithmetic is conservative — it
counted only *declaration* lines and still exceeded.

## Measured

`[MEASURED]` at candidate `2e4bba41`, nonblank lines in
`crates/bridge-worktree/src/sweep/checked_scan.rs`:

| Segment | Lines |
|---|---:|
| production (before `mod tests`) | 199 |
| test harness + helpers (`Script`, `sidecar`, `decoded_custody`, `temp_root`) | 108 |
| the four landed tests | 110 |
| **test module total** | **218** (cap: 220) |

Measured cost: **27.5 nonblank lines per test.** The harness is a one-time cost
already paid.

## Projection

| Row | Honest estimate | Cap | Ratio |
|---|---:|---:|---:|
| `checked_scan.rs` tests (108 harness + 19 × 27.5) | 630 | 220 | **2.9x** |
| `sweep.rs` tests (~9 × 27.5) | 248 | 85 | **2.9x** |
| **cumulative slice** | **~1,295** | **775** | **1.7x** |

The two evidence rows are each off by the same ~2.9x factor, which is the
signature of a systematic estimating error rather than one bad row. A 220-line
cap for ~20 named tests implies ~11 lines per test — unrealistic for tests that
build fixtures, inject a source, and assert a matrix. Nothing in the spec rounds
could have caught this: it only becomes measurable once real tests exist.

## What this actually reveals about the split

The A2/A2a split reduced **production** scope but left the **evidence** scope
nearly intact — A2a inherited almost the whole conformance matrix. The honest
projection of ~1,295 sits close to the pre-split A2 cap of 1,650, which is the
number the split was meant to escape.

So the split worked for the axis it was aimed at and not for the one that
actually dominates this slice.

## The production side is unaffected

`[MEASURED]` production is 199 lines in `checked_scan.rs` against a 230-line
row, plus `sweep.rs` routing — comfortably inside cap, accepted by both
reviewers, and host-gate green on fmt and clippy. Only the evidence rows are
mis-sized.

## Options

1. **Raise the evidence caps to the measured reality** (~1,300 cumulative) and
   proceed as one slice. Honest, but re-creates the big-bang review burden the
   split existed to avoid.
2. **Split the evidence** — land the three test fixes plus the decision matrix
   first, defer the remaining characterization scenarios to a sibling slice.
   Keeps each review round convergent; costs an extra cycle.
3. **Reduce required evidence** — drop scenarios from the matrix. Cheapest, and
   the least defensible: the decision matrix and classifier boundaries are the
   correctness claims this slice exists to preserve.

Recommend **2**, with the boundary at "correctness evidence" versus
"characterization evidence."
