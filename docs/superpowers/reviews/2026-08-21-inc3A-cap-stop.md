# 3A stopped at its cap — the stop was correct, and three fixes compounded

**Dispatch:** `impl-16985-bgwfhpdf`, base `84a48a4c` · **Outcome:** no candidate

## What happened

The agent stopped itself:

> Stopped on the task's mandatory cap gate: current Rust diff is 509 nonblank
> added lines, over the 500-line hard cap before formatting. Changes remain
> unstaged… No commit message was written, since this is not a valid
> implementation candidate without a split or explicit cap waiver.

`[MEASURED]` The stranded tree does not compile — 2 errors — because the agent
stopped **mid-work**. So 509 is a floor, not a total. Its work is preserved at
`scratchpad/inc3a-agent-work.patch` (936 lines) but is not a recoverable
candidate, unlike A2a-2's.

**This is correct behaviour.** The agent hit a declared stop condition, refused
to produce an invalid candidate, declined to stage, and said why.

## Three earlier fixes compounded to make this diagnosable in minutes

| Fix | What it gave here |
|---|---|
| **#64** clone pointer | `[implement] commit failed; clone left at …` — the path, where A2a-2 got nothing |
| **#64** stdout in the error | git's real `no changes added to commit` text, where A2a-2 got `git commit failed: ` |
| **#59/#61** persisted transcript | the agent's own reasoning, which is the whole diagnosis |

A2a-2's identical failure shape cost a full investigation and nearly destroyed
380 lines of correct work. This one was understood from three log lines and one
transcript read.

## The residual harness defect, second sighting

`stage_state` should have classified this tree `DirtyUnstaged` → `NoCommitDirty`
→ the friendly *"agent edited but staged NOTHING — NOT committing (agent owns
staging)"* message, with **no commit attempt**. Instead `decide` chose `Commit`
and the commit failed.

That is the defect PR #64 explicitly did **not** fix — it made the failure
legible and non-destructive without diagnosing it. This is its second sighting,
now with a clean reproduction in which the agent's non-staging was **deliberate
and documented**, which removes the earlier ambiguity about whether something
external emptied the index. Nothing emptied it; it was never filled.

The follow-up named in #64 stands and is now better justified: re-check
`stage_state` immediately before `host_commit` and fall through to
`NoCommitDirty` rather than attempting a commit that cannot succeed.

## The cap is the real finding — third consecutive miss

| Slice | Projected | Actual | Cap |
|---|---:|---:|---:|
| increment 2 | 455 | **673** | 670 |
| 3A | 420 | **509+** (floor, incomplete) | 500 |

Both misses are in the same direction, and 3A's projection was **grounded in
measured current-crate regions**, not guessed — sol measured the authority block,
`observe_exact_absence`, and increment 2's own tables, and still came in ~20%
low. The estimating model under-counts evidence systematically; the individual
projections are not careless.

## Disposition — split again, at an ordered seam

The owner chose to split rather than raise the cap. The seam is **not** the
obvious one.

3A's two changes look independent but are ordered. `source_common_dir_identity`
in `sweep.rs` shells to `git rev-parse --path-format=absolute --git-common-dir`,
and **four of the messages the retyping must map — the `source authority probe …`
family — live inside that function**. Retyping first would type them in a
location the port then relocates.

- **3A-1 — the port.** Move repository-authority observation behind
  `ExactAbsenceProbeV1` across all four implementations. Every message
  byte-identical; behaviour-preserving; no genuine behavioural red. Cap **220**.
- **3A-2 — the retyping.** Map every string error, now in its final location, to
  its typed object/reason pair, and remove the `.ok()` discard. Carries the
  behavioural red. Rebinds to 3A-1's accepted head.

Putting the port first also concentrates all of 3A's behavioural red in one half,
which makes each review answer one question instead of two.
