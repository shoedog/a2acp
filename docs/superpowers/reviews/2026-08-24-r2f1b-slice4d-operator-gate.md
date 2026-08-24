# R2f1b slice 4D — operator gate and custody evidence

Implementation candidate: `22cabad4` ("Settle the eight-arm scheduler arbitration order")
Base:                     `712fec68` (`origin/main`, R2f1b slice 4C)
Implementor:              `gpt-5.6-sol`, effort `xhigh`, review depth `thorough`
Loop:                     **1 attempt — converged**, `verify: PASS`, `review: APPROVE`

## The one-representation rule — operator-verified, not taken on report

This slice's entire deliverable is that the priority order exists **once**. That is precisely the kind
of property a review can wave through, so it was checked directly.

- Production carries a single ordered array, `SCHEDULER_ARM_PRIORITY_V1`, documented as "the sole
  executable representation of scheduler priority". Selection is
  `.into_iter().find(|arm| arm.is_ready(..))` over it.
- The test file references that constant **not at all**: no import of it, no sort of arms, no position
  lookup. Every row carries an explicit `expected: SchedulerArmV1::…`.

A test that re-derived the ordering would pass against a wrongly-ordered production. This one cannot —
which the mutation control then demonstrates rather than asserts.

## Invariants — checked against the tree, not the handoff

| Invariant | Result |
|---|---|
| `crates/bridge-workflow/src/executor.rs` untouched | **byte-identical** — path-scoped diffstat empty; the #22 bare await is intact |
| `Cargo.lock` / manifests untouched | no changes (the exception 4C required did not recur) |
| Cutoff tie is inclusive | `ready_at_ms <= absolute_cutoff_at_ms` |
| Unfinished nodes cancelled after the winning drain | `nodes_to_cancel_after_winner`, sorted |
| No clock read, spawn, sleep, select, or cancellation | arbitration takes caller-sampled offsets only |

## Operator gate — candidate `22cabad4`, idle machine

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
| `22cabad4` (candidate) | 0 | 0 |
| `712fec68` (base) | 0 | 0 |

Set difference empty in both directions.

## Frozen single-mutation control — independently re-measured

- Path: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4d-mutation-control.patch`
- SHA-256: `558057022e9ba2abc61d423a893945a767928c51bdb932e7d5d78a13b795de7f` — matches the handoff
- Mutation: **swap adjacent arms 1 and 2**, putting durable barrier acknowledgement ahead of the ready
  completion drain. Production only; a pure reordering of the single priority array.
- `git apply --check` clean; applied and reverted with the tree returning to `dirty=0`.

Newly red, computed as a set difference over a full-suite run (0 → 2):

```
completion_outprioritizes_durable_barrier_acknowledgement
scheduler_priority_table_is_exhaustive_and_all_ready_selects_arm_one
```

Two names, matching the handoff's two exactly. The mutated tree passes `clippy -D warnings` with
exit 0.

This control earns its keep: a **one-position** reordering of the array reddens the exhaustive table,
which is the direct demonstration that the table is data rather than a mirror of production.

## Handoff quality — the contract carried from 4C worked

4C lost a review round to a handoff that claimed green over a red run. 4D's spec required truthful
gate reporting and disclosure of hand-written fixtures. The handoff duly states:

- "All test fixtures and expectations are hand-written; no fixture generator was used."
- "Diagnostic runs made before honoring both environment requirements are not gate evidence" — naming
  a run that could not launch doc tests for want of `RUSTDOC`, and runs whose local fixture traffic was
  routed through the injected egress proxy.
- A transient compatibility process-status failure observed under a concurrent pass, with the isolated
  test then passing 10/10.

That is the right treatment of an inadmissible probe: reported, excluded from the evidence, and not
quietly folded into a green claim.

## Size

- Counted added nonblank Rust lines, `712fec68..22cabad4`: **388**
- Cap: **450**. Projection was 300.
- 126 production lines to 297 test lines — expected, since the deliverable is an exhaustive table.

## Residuals carried, not solved

1. **Post-cutoff completion drop is spec-silent and untested.** When the cutoff is reached,
   completions with `ready_at_ms` **after** the cutoff are dropped from the batch, so those nodes are
   cancelled instead. Defensible under "unfinished nodes are then cancelled", but the task never said
   it and nothing tests it. Inert today; it stops being inert the moment **4H** wires this to the live
   loop, which owes it either a test or an explicit decision. (Raised by the reviewers as the sole
   SMELL; the operator concurs.)
2. Carried from 4C and still open: `UnidentifiableCleanupOwnerProofV1` is sealed with no public
   constructor outside `bridge-core` — an integration risk for 4H.
3. Carried from 4C: `into_disposition`'s pending path is reconstruct-per-poll, not mutate-or-reuse.
