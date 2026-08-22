---
task-type: implement
---

# Author the T3b plan — the acting half

## Description

Write the **dispatch plan** for T3b. T3b is large enough that a single task spec
is the wrong artifact: your job is to define the slice sequence, each slice's
boundary and cap, and the spec for **the first slice only**.

T3a is complete and merged at `cafeae13`. It **decides and reports**: the sweep
returns a populated `ExactAbsenceSweepReportV1` with real root observations,
admits only candidate populations, types every claim-authority failure, and
leaves no vocabulary arm dormant.

**T3b acts.** It is the first slice in this lane that mutates — it transitions
custody state and unlinks. Everything T3a built is evidence for a decision T3b
makes under its own lock.

### The boundary that does not move

Every T3a spec has carried this, and it is now T3b's actual obligation rather
than a fence:

> The report carries **ordered historical evidence, not authority**. A later
> actor must **re-open, re-read, re-bind, and re-prove** exact absence under its
> own lock, regardless of what the report says.

T3b may not act on a stale report. A report produced before T3b takes its lock is
a hint about where to look, never a warrant. State this in the plan as a hard
rule with a test that would catch its violation.

### What T3b owns — scope items (c) and (d)

From the 3d dispatch brief, verbatim:

**(c) Candidate settlement.** Recovery-side `UnusedSettled` producer; the
async/trait recovery seam (**B18** — the sweep is sync, the registration probe is
async and private in `host_git`); boot-caller wiring; tri-state refusal
(present / absent / cannot-prove → **refuse**). A **refusing lock window**
across proof→transition→unlink (**B19**; contention tested in both orders; does
**not** activate the parked blocking-acquisition policy). **Descriptor-safe
removal (B20)**: same-object descriptor-relative transition-then-unlink,
no-follow, parent-synced, with crash-ordering and replacement/symlink negatives.

**(d) The 2b2 marker population.** Marker-removal authority keyed on the
state-agnostic exact-absence proof serves **both** populations, with **no
transition-table edge**.

**The mandated red-first battery:**
`unused_candidate_settles_only_after_exact_absence` (present target refuses;
registered-but-absent refuses; both-absent settles, marker only);
dropped-configure-future per phase; the finite-ownership row; contention in both
orders; replacement and symlink negatives.

### Sizing — plan the sequence before anyone implements

`[MEASURED]` **T3a delivered 4,769 nonblank added lines across nine slices** —
mean 520, max 800. The brief prices (c)+(d) undivided at ~2,000, but its prose
estimate for T3a's scope came in at roughly **half** of delivered, and the brief
itself instructs: *"For T3b: size from the delivered T3a delta, not from the
brief's prose."*

This lane's projection-to-delivered record, every projection grounded in measured
regions:

| Slice | Projected | Delivered | Ratio |
|---|---:|---:|---:|
| increment 2 | 455 | 673 | 1.48x |
| 3A (whole) | 420 | 509 (floor — stopped at cap) | ≥1.21x |
| 3A-1 | 80 | 105 | 1.31x |
| 3A-2 | 285 | 353 | 1.24x |
| 3B | 575 | 800 | 1.39x |

The model under-counts **evidence**, not implementation. Derive caps by applying
the worst observed ratio to a measured projection — that is how 3B's 850 cap was
set, and it held at 800 where the inherited 600 would have fired before a line
was written.

**Plan T3b as a sequence of slices each under ~800 delivered lines**, sequenced so
each is coherent alone and each later slice rebinds to the accepted head of the
one before. Measure per-test cost in `crates/bridge-worktree` yourself. If your
honest total exceeds the brief's ~2,000, say so — that is a finding, not a
failure.

### Safety framing this slice earns

T3b unlinks. Three consequences the plan must address explicitly:

1. **The first slice must not be the one that unlinks.** Sequence the proof, the
   seam, and the lock window ahead of any destructive edge, so the destructive
   slice lands last on a foundation already reviewed and merged.
2. **`EXACT_ABSENCE_POLICY_READY_V1` is the sole remaining production gate.**
   Since slice B, `has_authoritative_scan()` returns `true` for a healthy root, so
   `entry_is_effectively_authorized_for_policy` is single-gated. `effective()`
   currently has **no production consumer**. If T3b becomes that consumer, say in
   which slice, and treat flipping readiness as its own reviewed decision with its
   own red control — not a line in a larger change.
3. **Tri-state refusal is fail-closed.** `cannot-prove` refuses. An observation
   that could not be completed is never evidence of absence.

### A recurring defect to avoid

`[MEASURED]` The `## Commit Message` section defect has recurred **three times**
in this lane and is root-caused: the typed task-spec schema treats that entire
section as the message, so any instruction prose inside it becomes the commit
subject. Once a fence produced a bare ```` ``` ````; once a warning sentence
became the subject.

**The section must contain the message and nothing else.** Guidance belongs
elsewhere in the document.

### Environment facts

Your working tree is at `main` (`cafeae13`), and the repository is authoritative
over every claim above. Several operator claims in this lane have been refuted by
authors reading the code — including one that pointed a gate at a site where it
could not suppress anything. Verify each anchor: the `UnusedSettled` state and its
current producers, `custody_writer`'s frozen transition table, `remove_worktree_if_safe`
and the two forgery guards that precede it, `host_git`'s registration probe, and
what `effective()` currently yields.

You cannot read anything outside the repository, and no spec you emit may name a
path outside it — implementers run in a container with only the code tree mounted.

## Acceptance Criteria

This authoring task is complete when you emit:

1. **A slice plan** — an ordered sequence with, for each slice: its boundary, what
   it owns, its measured projection, its derived cap, why it is coherent alone,
   and what it rebinds to. The destructive slice must not be first.
2. **A full task spec for slice 1 only**, dispatchable as-is, with front matter
   `task-type: implement` and sections `## Description`, `## Acceptance Criteria`,
   `## Files`, `## Spec Refs`, `## Commit Message` — that last containing the
   message and nothing else.
3. **The re-prove rule** stated as a hard requirement with the test that catches
   its violation.
4. **A statement on readiness** — whether T3b flips `EXACT_ABSENCE_POLICY_READY_V1`,
   in which slice, and under what evidence.
5. A falsification license: the repository is authoritative, and an implementer
   who finds a stated anchor false must stop and report rather than adapt.

## Output contract

Emit the slice plan followed by the slice-1 task spec, as one document.

Prefer precision over length. This is the slice that deletes things; a plan an
implementer can execute without guessing is worth more than a thorough one that
restates context.
