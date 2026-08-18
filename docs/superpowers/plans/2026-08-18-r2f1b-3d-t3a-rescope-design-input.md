---
task-type: design
---

# R2f1b 3d T3a — re-cut slice 1's boundary, given two failed spec reviews

## Description

You previously produced a design splitting T3a's residual into two slices. Slice
1's task spec has now failed **two** counted spec reviews and is parked at its
declared cap. **Re-cut the boundary.** Do not re-derive the whole design, and do
not defend the previous one — the question is narrow: *what is the smallest first
increment that can actually be specified, implemented, and evidenced?*

The repository at the session cwd is checked out at `main` = `9aedf175`.

### What the two review rounds actually showed

Round 1 returned 7 blockers / 12 findings. Round 2, after all 12 were folded,
returned **8 blockers / 13 findings** — not fewer, not smaller, and several
repeating in kind. The operator classified this **open-class** under the standing
convergence rule and parked rather than extending to a third round.

The repeating kind is this: **the spec is being asked to pin down, in prose, an
exhaustive typed enumeration.** Every `(state, claim presence)` pair with no
wildcard; every degraded-claim field outcome across source, root, worktree and
common-directory identity; every guard's refusal variant and its precedence
relative to the others. Each round the prose grew and each round the reviewer
correctly found another pair the prose had not covered. That is a job the compiler
does for free and prose does badly.

### Two findings that are about FORM, not wording — both operator-verified

These are the reason this is a re-scope rather than a third spec round.

**F1 — the production path cannot carry a typed result.**
`sweep_orphans_with_exact_absence` returns `()` and merely
`tracing::info!(record, ?decision, ...)`s each outcome. The previous design's exit
gate requires all four arms to assert **exact typed assessments through the real
production traversal**. That is impossible against the current signature. A typed
collector or reporting seam shared by production traversal and tests has to exist
first, and the previous design did not account for it.

**F2 — new vocabulary cannot be behaviorally red on base.** Any test naming a new
type such as `IneligiblePopulation` does not *fail* on `9aedf175` — it does not
**compile**. Treating a compile error as the mandated pre-change failure is
inadmissible evidence, and this lane has a standing rule against it. So a slice
that introduces new vocabulary *and* demands behavioral red evidence for tests
written in that vocabulary is self-contradicting as specified.

### Other unresolved defects the re-cut must design away, not restate

- **Ownership vocabulary is incoherent as specified.** The spec required a
  recovery-ownership refusal path to stay *reachable* while `LocallyOwned` and
  `OwnershipCannotProve` are *constructed nowhere*. An implementer must then either
  make the required path unreachable, construct a forbidden variant, or keep an
  undocumented wrapper. Decide: define the ownership input and assessment signature
  now, or defer both the variants and the reachability wholly to the later slice.
- **Guard refusals have no variants or precedence.** `worktree_under_root` and the
  canonical record-file/sibling check both must survive, but an outside-root record
  could map to either `IneligiblePopulation` or `CannotConstructSubject`, and the
  order between the two guards is unspecified.
- **Guard and zero-probe fixtures can false-green.** Zero probe calls prove nothing
  when `from_claim` rejects synthetic source/common-directory authority *first*; a
  naive outside-root fixture may also trip the sibling guard, so deleting only the
  intended guard stays undetected.
- **Effect-freedom evidence is too weak.** Before/after snapshots prove final-state
  equality only; they cannot exclude a transitive helper that mutates and restores.
- Minor but true: "every claim-bearing V3 record reaches the proof" is imprecise —
  only every *constructible* claim-bearing record attempts it.

### What has not changed

- **T3a decides; T3b acts.** Not open for revision.
- **No record mutation on any T3a path**, and no new edge in the frozen custody
  transition table.
- The real defect still to close: `decide_unused_custody_record` constructs its
  candidate from `record.claim` with **no check on `record.state`**, so a
  `Preserved` record whose target vanished externally can produce a positive
  result. `ProtectionPrepared` is the schema's one `ClaimPresenceV1::Optional`
  state, so its claim-bearing form is valid and constructible too.
- The three owner decisions stand: bare `ProtectionPrepared` residue refuses
  without any durable-schema work here; the eligible population is assessed without
  inferring pre-target status from a state name; and no test's API is contorted
  purely to force a red.
- Fail-open asymmetry: a wrong positive becomes destructive under T3b; a wrong
  refusal merely declines.

### Falsification license

Every claim above is an operator claim measured at `9aedf175`, including F1 and
F2, and the repository is the authority. If `sweep_orphans_with_exact_absence`
does return a typed value, if some vocabulary already exists on base, or if a
defect listed here does not exist — **say so with the evidence and drop it.**
Concluding that the previous two-slice boundary was right and only the spec was
wrong is a permissible answer *if* you can show how to evidence it; but you must
then answer F1 and F2 concretely rather than restating the requirement.

## Acceptance Criteria

1. **Re-cut the boundary.** Propose the sequence of increments that actually gets
   this shipped, each independently landable, independently green, and reviewable
   within a two-round cap. For each, state the line budget and the exit gate. If
   the first increment is a behavior-preserving change, say so plainly and say what
   makes it worth landing alone.
2. **Answer F1 concretely**: the exact shape of the typed collector/reporting seam,
   which function signatures change, who consumes it in production versus tests,
   and whether it lands before or with the admission gate. Name the options
   rejected.
3. **Answer F2 concretely**: for each proposed increment, say what its genuinely-red
   behavioral evidence is, given that tests naming new types cannot compile on the
   base of the increment that introduces them. If an increment has no available
   behavioral red, say so and say what stands in its place — do not assign
   evidence that cannot exist.
4. **Resolve the ownership question**: define the ownership input and assessment
   signature now, or defer variants and reachability entirely. Not both.
5. **Assign guard refusal variants and an explicit precedence** between the
   under-root and record-file/sibling checks, and describe fixtures that isolate
   each guard — including how a fixture avoids tripping the other guard, and how a
   zero-probe assertion is protected from false-greening when `from_claim` rejects
   synthetic authority first.
6. **Specify the degraded-claim matrix** across source, root, worktree and
   common-directory identity: which combinations are constructible, which already
   refuse before probing today, and what each must yield. This is the enumeration
   prose kept failing to cover — decide whether it belongs in the design, in code
   as an exhaustive match, or in a table-driven test, and say why.
7. **Say what evidence effect-freedom actually requires** — an operation-recording
   seam, a transitive call-graph audit, or something else — and scope it to
   something an implementer can complete.
8. **State explicitly what the compiler should enforce rather than the spec.** The
   failure mode being designed away is prose enumerating what an exhaustive `match`
   would enforce for free.

## Spec Refs

Not present in this checkout — reproduced above where load-bearing, and their
absence is not a missing input:

- `docs/superpowers/plans/2026-08-18-r2f1b-3d-t3a-rebuild-design.md` — your previous
  design.
- `docs/superpowers/plans/2026-08-18-r2f1b-3d-t3a-slice1-task.md` — the twice-rejected
  slice 1 spec.
- `docs/superpowers/reviews/2026-08-18-t3a-slice1-spec-review.md` and
  `...-round2.md` — the two verdicts.

In this checkout and authoritative: `crates/bridge-worktree/src/sweep.rs`,
`.../custody.rs`, `.../host_git.rs`, `.../backend.rs`.
