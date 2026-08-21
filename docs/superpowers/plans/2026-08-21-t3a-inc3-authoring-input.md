---
task-type: implement
---

# Author the increment 3 task spec — typed claim authority and root-bracketed Host Git

## Description

Write a complete, dispatchable **task spec** for T3a increment 3. Not a design
note — a spec an implementer can execute.

Increment 2 is merged. The sweep now admits only two of sixteen custody
populations and types the three placement guards. But when an admitted record's
claim fails to construct, production still collapses that failure to a bare
`Refused` and reports it as `Assessed`, discarding why — and
`ClaimAuthorityUnavailable` remains dormant in production, deliberately left so
by increment 2 because this increment owns it.

### What the design assigns to increment 3

From `docs/superpowers/plans/2026-08-18-r2f1b-3d-t3a-rescope-design.md`:

- Replace string-valued claim-construction errors with the closed typed mapping.
- Move all Git authority observation behind `ExactAbsenceProbeV1`.
- Retain and bracket the custody-root identity.
- Add the 16-row degraded matrix, stale-object rows, and real persisted-record
  Host Git tests.

Its named behavioral-red evidence against increment 2:

- a degraded root currently reaches the probe when source and common directory
  are valid; it must become `ClaimIdentity(Root, Degraded)`;
- root replacement during Git can currently yield `BothAbsent` for the
  replacement root; it must refuse;
- wrong-repository / common-directory substitution must refuse;
- degraded and historical-complete worktrees with complete required authority
  must still reach the probe;
- every regression asserts unchanged custody bytes.

### The string errors that must be retyped

`ExactAbsenceCandidateV1::from_claim` and its `from_bound` return
`Result<Self, BridgeError>` carrying prose. Verify each site and its message
against the code, then map it. The sites are approximately:

- per-object path/identity disagreement, for `source`, `source common directory`,
  and `worktree`;
- `capture_directory_identity` failing for source or common directory;
- source or common-directory identity changed, including the binding check
  against `source_common_dir_identity`;
- worktree path not absolute;
- source or common directory carrying no bound object identity.

The typed vocabulary **already exists** in `crates/bridge-worktree/src/sweep/report.rs`
and must not be extended:

- `ClaimAuthorityObjectV1`: `Source`, `Root`, `Worktree`, `CommonDirectory`,
  `SourceCommonDirectoryBinding`
- `ClaimAuthorityUnavailableReasonV1`: `PathMismatch`, `NotAbsolute`,
  `IdentityIncomplete`, `ObservationUnavailable`, `IdentityChanged`,
  `OwnershipUnproven`

Two of those — `Root` and `OwnershipUnproven` — are **not** producible from
`from_claim` today. They belong to the root-retention half of this increment.
Say which arms each half produces, and do not leave any arm unaccounted for: an
arm no path can construct is dead vocabulary and must be called out as such
rather than silently tolerated.

### Where the failure is discarded today

In `sweep.rs`, the admitted-record tail does
`ExactAbsenceCandidateV1::from_claim(..).ok()` and then
`.unwrap_or(UnusedCandidateDecisionV1::Refused)`, reporting
`Assessed(Refused)`. That `.ok()` is the discard. Increment 2 recorded this as a
known imprecision — a construction failure reported as `Assessed` although
nothing was assessed — and deferred it here.

### Sizing — do not inherit the 600 anchor

The design sets a hard cap of **600 changed lines including tests**. Treat that
as an anchor to test, not a budget to trust.

`[MEASURED]` increment 2's spec estimated 455 and the delivered artifact counted
**673** against a 670 trigger — its evidence rows missed by nearly 2x, with a
recording-probe fixture at 137 against a 75 cap and a sixteen-population table at
150 against 115. The caps under-estimated evidence, not the implementation.

Increment 3 carries a 16-row degraded matrix, stale-object rows, and real
persisted-record Host Git tests, which is at least as much evidence surface.
Measure the per-test cost in this crate yourself and size from it. If your honest
estimate materially exceeds 600, **say so and propose a split** rather than
compressing evidence to fit. The design itself says a projected cap breach is
split before implementation, never excused after review — and this lane has now
split twice, both times correctly, and once overridden a breach after the fact,
which is the outcome to avoid repeating.

### Scope fences

Increment 3 does **not**:

- set `EXACT_ABSENCE_POLICY_READY_V1` to `true`, or change `effective()` or
  `entry_is_effectively_authorized_for_policy`. `effective()` has no production
  consumer, and since slice B readiness is the **sole remaining gate** — flipping
  it belongs with the slice that consumes it, T3b;
- change the population admission table or guard precedence landed by increment 2;
- add or remove any arm of `ClaimAuthorityObjectV1`,
  `ClaimAuthorityUnavailableReasonV1`, `IneligiblePopulationV1`, or
  `CannotConstructSubjectV1`;
- add ownership, locking, transition, unlink, or removal authority. **T3a decides
  and reports; T3b acts.** A later actor must re-open, re-read, re-bind, and
  re-prove exact absence under its own lock;
- repair the Unix-only separator guard in `is_custody_record_name`, or the
  non-latching entry-error loop — both carried forward deliberately.

### Behavior that must not change

Every test landed by A2a-2, A2b, slice B, and increment 2 must still hold. Where
an existing assertion legitimately changes because a construction failure now
reports typed authority instead of `Assessed(Refused)`, **name each such test and
justify it individually**. An unexplained colour change elsewhere is a behavior
change: stop and report.

### Operator-owned gates

The implement container's egress cannot fetch the pinned `a2a-lf` dependency, so
`cargo` cannot build there. The implementer makes the implementation-candidate
commit and authors a handoff carrying six unticked `PENDING OPERATOR` gate lines;
the host operator runs the gates and makes the handoff-only evidence commit.
Require a frozen genuine-red control — a test-only patch against a recorded base
tree, its SHA-256, and the run command — as increment 2 and slice B both did.
Increment 3's red is **behavioural**, not a compile barrier; say so, and require
the control to demonstrate that.

### Environment facts

Your working tree is at `main`, and the repository is authoritative over every
claim above. Several claims in earlier briefs for this lane were false and were
caught by authors reading the code — including one that would have installed a
gate where it could not suppress anything. Verify each anchor: `from_claim` and
`from_bound`'s exact error sites, the `.ok()` discard, the typed vocabulary's
arms, `observe_exact_absence`'s current Git observation, and
`revalidate_source`.

You cannot read anything outside the repository, and the spec must never name a
path outside it — the implementer runs in a container with only the code tree
mounted.

## Acceptance Criteria

This authoring task is complete when the emitted spec: maps every string-valued
claim error to a typed object/reason pair with no arm left unaccounted; states
what "move Git authority behind the probe" concretely requires; specifies the
root retention and bracketing, and which arms it produces; defines the 16-row
degraded matrix; names the tests whose assertions legitimately change; requires a
behavioural frozen control; and gives a per-row sizing worksheet measured from
this crate with a falsification license.

## Output contract

Emit the complete increment 3 task spec, with front matter
`task-type: implement` and sections `## Description`, `## Acceptance Criteria`,
`## Files`, `## Spec Refs`, `## Commit Message`.

Prefer precision over length. A spec an implementer can execute without guessing
beats a longer one that restates context.
