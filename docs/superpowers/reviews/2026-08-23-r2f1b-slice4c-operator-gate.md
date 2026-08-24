# R2f1b slice 4C — operator gate and custody evidence

Implementation candidate: `306f5bda` ("Settle preservation and exact cleanup ownership")
Base:                     `1b328196` (`origin/main`, R2f1b slice 4B)
Implementor:              `gpt-5.6-sol`, effort `xhigh`, review depth `thorough`

## How this slice was run — and why it differs from 4A/4B

4C hit its declared 3-attempt bound. Rather than extend the coupled loop a fourth time, the operator
**decoupled it** at the owner's direction: two standalone `implement-fix` turns via
`run-workflow` (which stage but never commit, so the operator inspects and folds), with an
**independent** `implement-review` run in between whose verdict gated nothing.

| Round | Verdict | Finding class |
|---|---|---|
| 1 | REJECT | Late `Complete`/`Failed` skipped the deadline-transfer rule; `UnsettledUnknown` could encode two contradicting owners |
| 2 | REJECT | `NotNeeded`+`Preserved` constructible but rejected downstream; ownerless-`Unknown` selectable when the owner *was* identifiable |
| 3 | REJECT | Code sound per both reviewers; `verify` red on one `bridge-api` test; handoff falsely claimed green |
| fix 1 | — | Handoff corrected to record the red gate and the attribution evidence |
| independent review | REJECT (advisory) | Required test #5 never built as specified; the green-gate criterion |
| fix 2 | — | trybuild compile-fail proof added; cosmetic test-module fold |

**The operator's mid-slice read was wrong and is recorded as such.** After round 2 the operator judged
the findings *open-class* and recommended parking and re-splitting 4C. Round 3 fixed both round-2
MAJORs and both reviewers confirmed the design sound. The findings converged; the recommendation was
premature.

## Owner waiver — the `bridge-api` flake

`verify` failed at `test` with exactly one failing test,
`bridge-api backend::tests::settlement_refusal_does_not_mask_the_provider_failure`. The reviewers
correctly refused to attribute it without a control. The operator ran one:

| Environment | Base `1b328196` | Candidate |
|---|---|---|
| Same `a2a-toolchain:latest` container, under compile contention, 10 runs | 10/10 ran and passed | 10/10 ran and passed |
| Host, idle, 25 runs | 0 failed | 0 failed |
| Host, under compile contention, 10 runs | 0 failed | 0 failed |

The decisive row is the candidate's container result: **the same test, on the identical tree, in the
same image, passed 10/10** where verify saw it fail. A tree cannot both deterministically break a test
and pass it ten times, so the failure is **non-deterministic**. `crates/bridge-api` is also untouched
by this diff — the path-scoped diffstat against the base is empty.

**Stated limit:** the failure was never reproduced, so "pre-existing" is an inference from
non-determinism plus untouched code, not a demonstrated base-tree failure.

**Waiver: approved by the owner, 2026-08-24.** It is belt-and-braces in the event: the host gate below
is fully green, so the green-gate criterion is met on the binding evidence regardless.

### A probe that proved nothing, recorded so it is not repeated

An earlier operator attempt to re-run the whole verify command in a container exited 101 with five
failures — and the `bridge-api` test **was not among them because it never ran**. The verify command
carries no `--no-fail-fast`, so cargo stopped at an earlier binary that failed for the probe's own
reasons (no egress network/proxy). Reading that as "bridge-api passed" would have been exactly
backwards. The replacement probe counts `confirmed-ran-and-passed` by grepping the actual `... ok`
line, so a run that never executes the test counts as neither pass nor fail.

## Operator gate — candidate `306f5bda`, idle machine

| Gate | Exit |
|---|---|
| `cargo fmt --all -- --check` | 0 |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 |
| `cargo test -p bridge-core --locked --no-fail-fast` | 0 |
| `cargo test -p bridge-workflow --locked --no-fail-fast` | 0 |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` | 0 |
| `cargo test --workspace --locked --no-fail-fast` | **0 — zero failures** |

## Attribution control — same environment, same machine, sequential

| Tree | Workspace exit | Distinct failures |
|---|---|---|
| `306f5bda` (candidate) | 0 | 0 |
| `1b328196` (base) | 0 | 0 |

Set difference empty in both directions. Note for the record: slices 4A and 4B measured **9**
pre-existing host failures in `tests/smoke_cli.rs` and `tests/fallback_plan_cli.rs`. Both trees now
report zero, so that population was environmental and has since cleared — which is itself a reason to
re-measure the base every time rather than carry a remembered number forward.

## Frozen single-mutation control — independently re-measured

- Path: `docs/superpowers/reviews/2026-08-23-r2f1b-slice4c-mutation-control.patch`
- SHA-256: `8c591a02e2393dfd3fd806111ace03fefbaa47aef25cdeb5f30d7e681b501d31` — matches the handoff
- Mutation: `elapsed >= deadline` → `elapsed > deadline`. **Production only**; an off-by-one on the
  exact boundary the transfer rule turns on.
- `git apply --check` clean; applied and reverted with the tree returning to `dirty=0`.

Newly red under the mutation, computed as a set difference over a full-suite run (0 → 4):

```
preservation_first_disposition_transfers_the_guard_owner
unknown_cleanup_retains_identifiable_owner
unsettled_at_deadline_transfers_exact_owner_as_partial
work_cutoff_plus_cleanup_tail_cap_binds
```

Four names, matching the handoff's four exactly — an independent reproduction. The mutated tree
passes `clippy -D warnings` with exit 0.

## The compile-fail proof — verified, because it was hand-written

Required test #5 (a disposition is unobtainable before preservation is typed) was **not** delivered in
round 3; the shipped test always called `.after_preservation()` first and so proved a different,
pre-existing coherence property. The independent review caught this and pointed at the repo's own
precedent (`trybuild`, used in `bridge-core`, never applied to `bridge-workflow`).

The added fixture was **hand-normalized rather than generated with `TRYBUILD=overwrite`**, because the
agent's environment could not reach the registry. trybuild compares byte-for-byte, so the operator ran
it:

```
test tests/trybuild/disposition_before_preservation.rs ... ok
test result: ok. 1 passed; 0 failed
```

The case proves `into_disposition()` on `WorkflowNodeCancellationSettlementV1<PreservationRequiredV1>`
fails with `E0599`, and the diagnostic itself records that the method exists only for
`PreservationTypedV1`. The ordering invariant is now enforced by the compiler, not asserted by a test
that could not fail.

## Size

- Counted added nonblank Rust lines, `1b328196..306f5bda`: **498**
- Cap: **500**. Projection was 350.
- The operator predicted the trybuild addition would breach the cap by ~3 lines. It did not: a
  now-redundant local type-gating helper was removed from the runtime test, since the compile-fail
  case supersedes it.

## Exception to a stated invariant

**`Cargo.lock` is modified** — one line, `trybuild` added to `bridge-workflow`'s dev-dependencies. The
slice's invariants said the lockfile was untouched; adding the compile-fail proof the review required
makes that unavoidable. Diff inspected to confirm it is that line and nothing else.

## Residuals carried, not solved

1. `UnidentifiableCleanupOwnerProofV1` has a private field and no public constructor, so only
   `bridge-core` can build the ownerless-`Unknown` observation. Consistent with 4C's seam-only scope,
   but a **forward integration risk for 4D/4H** — whoever wires the multiplexer will need a
   constrained constructor.
2. `into_disposition`'s `Err(Box<Self>)` pending path freezes `elapsed_after_cancellation_ms` at
   construction with no accessor, so the seam's contract is reconstruct-per-poll, not mutate-or-reuse.
   Correct for an isolated seam; flagged for the 4H author.
3. The settlement decision is **not wired into the live cleanup path** — deliberate, per the task's
   own scope. Integration lands with 4D–4H.
