# A1 dispatch produced no changes — investigation

**Date:** 2026-08-18 · **Run:** `impl-18576-2v9yqkr3` · **Base:** `main` `9aedf175`

## Observation

`a2a-bridge implement` exited 0 with `implement: made no changes`. The clone is
clean at the base commit. No `implement-checkpoint.json`, no handoff, no review
slices — where the last successful run (`impl-34727-h02eihj9`) wrote a checkpoint.

The brief WAS delivered: `.git/A2A_TASK.md` is present at 68,643 bytes and contains
the spec including its mandatory pre-edit stop. `lsp warm-deps` reported ok, so the
container came up.

## Measured comparison

| Spec | Lines | Bytes | Outcome |
|---|---:|---:|---|
| `2026-08-17-r2f1b-3d-t3a-task.md` | 192 | 9,796 | drove an implement |
| `2026-08-18-path-identity-repair3-task.md` | 225 | 12,505 | drove an implement, APPROVE first pass |
| `2026-08-18-path-identity-repair2-task.md` | 380 | 22,047 | drove an implement |
| `2026-08-18-...-inc1-sliceA-task.md` | **1,543** | **68,672** | **no changes** |

The failing spec is 3–7× larger than anything that has worked in this lane, and
contains 49 references to A2 — content the A1 implementer cannot act on.

## Hypotheses

**H1 — size.** The brief is too large to act on; the agent consumed it without
producing edits. Supported by the size comparison above and by the absence of any
checkpoint, which suggests no productive turn completed.

**H2 — the spec's own pre-edit stop fired.** The spec mandates: produce a component
estimate before editing, and if it exceeds the 700-line cap, stop and propose a
split. An agent obeying that would make no edits. Weakened, but not excluded, by the
absence of any written proposal or handoff.

**Not discriminable from the retained artifacts.** No agent transcript is kept for
this run. Recording the gap rather than asserting a cause.

## Finding

Review convergence and dispatch fitness are different properties, and this lane
optimized the first at the cost of the second. Six review rounds each added
precision — literal declarations, an exhaustive audit inventory, a characterization
matrix, an A2 outline — and the cumulative artifact became correct and unusable. No
gate tracked the total, because every individual addition was justified.

## Remedy

Identical under both hypotheses: an implementer-facing brief scoped to A1 alone,
sized like the specs that have actually driven implements. The reviewed document
remains the normative record; the dispatch brief is derived from it.

## Process change proposed

Cap the dispatched brief, not just the diff it produces. A spec over ~400 lines
should be treated as a dispatch risk on this lane's evidence, and split or condensed
before dispatch rather than after a null run.
