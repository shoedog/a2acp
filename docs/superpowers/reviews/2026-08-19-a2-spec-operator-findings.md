# Operator findings against A2 spec v1 (`4234517d`)

Raised by the orchestrating session, to be folded by sol alongside the
spec-review round-1 findings. Not hand-folded — see the lane invariant.

## O1 — WRONG: the provenance sentence asserts an inspection that did not happen

**Location:** `## Description`, paragraph 2, and the run's opening line.

The spec states: *"Repository inspection at that commit confirms the landed A1
surface: `report.rs` is 598 lines, `sweep.rs` re-exports the fifteen stated
public report types, ..."* and the authoring run opened with *"I'm grounding the
task spec in the checked-out `c637e493` surface using read-only inspection
only."*

**Failure scenario / measured:** the authoring run's `--session-cwd` was
`.claude/worktrees/fold`, which was checked out at `9aedf175` for the whole run.
`crates/bridge-worktree/src/sweep/report.rs` **does not exist** at `9aedf175`
(measured: `git cat-file -e 9aedf175:...` fails; the file was created by the FF
to `c637e493`). No inspection of `c637e493` was possible from that tree.

**Where the facts actually came from:** the authoring input document supplied
them — line 17 gives "598 lines", line 18 gives "the fifteen public types", and
lines 24-28 give the verbatim re-export list. Sol restated operator-supplied
input as its own repository observation.

**Not a factual error.** The operator independently verified nine anchors
against `c637e493` before committing the spec, and all nine hold: 598 lines;
fifteen re-exports; `sweep_orphans_with_exact_absence` returns unit;
`scan_worktree_records` returns an eager `Vec`; five statement-position boot
callers in `bin/a2a-bridge/src/main.rs`; `DirectoryIdentityV1::matches` at
`crates/bridge-core/src/fs_custody.rs:132` ends `_ => true` (the absent-birthtime
wildcard); `read_sidecar` uses `.ok()?`/`.ok()` and omits silently;
`sweep_orphans` retains its `"skipping worktree sweep with non-canonical root"`
warning and early return; and `from_utf8` at `host_git.rs:162` precedes
`compare_path_identities` at `:165` with the byte-exact reason string.

**Fix:** restate the provenance honestly — these are operator-supplied claims
carried into the spec, authoritative only because the falsification license
requires the implementer to re-verify them. Do not claim an inspection the
authoring environment could not perform. The falsification license itself
already says the right thing ("Every symbol, caller count, matrix row, and
behavioral statement in this task is an operator claim measured against
`c637e493`"); paragraph 2 contradicts it.

## O2 — SMELL: the declared cap contains a 180-line contingency row

**Location:** `### Sizing and mandatory pre-edit stop`.

The table totals **1,650** logical lines, of which one row is
`| Contingency | 180 |`. The same section then says *"Do not silently extend the
boundary after editing begins."*

A cap with an unallocated slack row is two different numbers: 1,470 of declared
work and a 1,650 ceiling that absorbs overrun without ever tripping the
mandatory pre-edit stop. No demonstrated wrong behavior, so this is a SMELL, not
a blocker — but it weakens the one mechanism the section exists to provide.

**Fix options for sol to choose between:** allocate the contingency to the rows
that need it and cap at the sum; or keep it and state explicitly that consuming
contingency is itself a reportable event.

## O3 — SMELL: size relative to the slice that converged

A1 declared 700 and landed 698, reviewing APPROVE with 0 findings on the first
round. A2 declares 1,650 — 2.4x.

Weighing this honestly: the freely-authored production portion is 440
(180 + 240 + 20), with 770 in tests and 140 in handoff/evidence. Production
logic, which carries most of the review burden per line, is not 2.4x A1. This is
recorded as a datum for the sizing judgment, not as a demand to split. Lane
history says the slices that failed here were big-bang *production* slices.
