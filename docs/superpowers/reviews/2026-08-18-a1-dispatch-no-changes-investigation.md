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

**Not discriminable from the retained artifacts** — and the reason is a bridge gap,
not bad luck. See "Observability gap" below.

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

## Observability gap — the implement agent leaves no transcript

`MEASURED`. Host-side agents do leave transcripts: `~/.codex/sessions/2026/08/18/`
holds rollouts at 23:04, 23:07 and 23:08, which are the three nodes of the
spec-review workflow that ran immediately before this dispatch. The implement run
wrote its brief at 23:09 and left **no rollout at all**.

The cause is structural. Reviewers run host-side, so codex writes its rollout into
the host `CODEX_HOME`. The implement agent runs `codex-acp` inside a `--rm`
container whose only `.codex` mount is
`/Users/wesleyjinks/.config/a2a-creds/codex/auth.json:/root/.codex/auth.json`, so
`/root/.codex/sessions/*.jsonl` is created inside the container and destroyed with
it. `git grep` over `bin/a2a-bridge` and `crates/` finds **no** provision that
mounts, copies out, or otherwise persists it.

Consequence: **a null implement run is currently undiagnosable.** The only survivors
are the bridge's own artifacts under the clone's `.git/a2a-bridge/`, and on a run
that produces no turn even the checkpoint is absent.

### The pattern to copy already exists

The same impl agent config deliberately routes the lsp-mcp call log to
`{cwd}/.git/a2a-bridge/lsp-mcp-calls.log` with the comment that it "lands under the
clone's `.git/` (survives `--rm`, fetched at hand-off)", and `main.rs` already
appends per-clone named volumes rather than static ones. The agent's own session
rollout simply was not given the same treatment.

**Proposed fix:** mount a per-run host path at `/root/.codex/sessions` (or otherwise
land the rollout under the clone's `.git/a2a-bridge/`) using the existing per-clone
volume mechanism. Prefer the mount over repointing `CODEX_HOME`, since `CODEX_HOME`
also anchors the mounted `auth.json`.

This is a bridge defect worth its own slice, independent of T3a. Its absence is why
the two hypotheses above cannot be separated.
