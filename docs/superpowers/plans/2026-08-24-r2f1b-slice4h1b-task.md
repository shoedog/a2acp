---
task-type: implement
---

# R2f1b slice 4H-1b — mint the ownerless-cleanup proof instead of validating it

## Description

Slice 4H-1 withdrew a requirement that could not be satisfied in the shape it was written. This task
replaces it with one that can.

**The failed shape.** 4H-1 item 5 asked for a "constrained constructor" for
`UnidentifiableCleanupOwnerProofV1` — `from_cleanup_deadline_transfer_v1(&CleanupDeadlineTransferV1)`
— that would inspect a caller-supplied value and decide whether it constituted evidence. Three review
rounds found three distinct forgery routes: the public fields of `CleanupDeadlineTransferV1::Unknown`;
a caller-chosen foreign guard; and a durably-journaled `Settled{Unknown, recovery_owner: None}` row,
reachable either by two ordinary sequential `transfer_cleanup_deadline` calls or by pre-seeding
through the public journal trait.

**Why it could never work.** "Ownership is genuinely unidentifiable" is a claim about **provenance**.
Provenance cannot be established by inspecting a value: any value a caller can obtain, a caller can
obtain again by another path. Validating a supplied value is the wrong shape, not a shape that needed
a better validator.

**The replacement shape.** `bridge-core` **mints** the proof at the point where it itself observes the
condition, and hands it out. There is no constructor to call. Holding a proof means having been given
one.

Base: `origin/main` = `9ade2c49` (R2f1b slice 4H-1).

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md`.
Prior evidence: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h1-operator-gate.md`.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails on the base tree:

- `UnidentifiableCleanupOwnerProofV1` is a tuple struct with a private `ResourceFlightIdV1` field and
  **no public constructor and no `impl` block**.
- `NodeCleanupObservationV1::UnsettledUnknownOwnerless(u64, UnidentifiableCleanupOwnerProofV1)` exists.
- `RetainedResourceFlightV1::transfer_cleanup_deadline` returns
  `Result<CleanupDeadlineTransferV1, RetainedResourceFlightError>` and has **three** branches that
  yield `CleanupDeadlineTransferV1::Unknown`.

**Do NOT stop for immaterial measurement differences** — line numbers, diff counts, formatting-only
deltas. Cite by symbol, never by line.

### Verified anchors — operator-measured on this base

`transfer_cleanup_deadline` reaches `Unknown` by three distinct routes, and they are **not**
equivalent:

1. **Foreign guard** — `!Arc::ptr_eq(self, &guard.flight)`. The caller passed a guard belonging to
   another flight. This is caller error, not an observation about ownership.
2. **Guard token not held** — `!state.guards.contains(&guard.token)` under the lock. This call
   established, against its own live state, that the guard is no longer held.
3. **Adopted durable terminal** — `adopt_durable_terminal_locked` returned an existing settled record.
   The record's **provenance is unknown**; it may have been journaled by anything, including a
   pre-seed through the public journal trait. This is precisely the round-3 forgery route.

## What this sub-slice does

**1 — Mint, never validate.**

`UnidentifiableCleanupOwnerProofV1` keeps **no public constructor**. The only way to obtain one is to
receive it from `transfer_cleanup_deadline`, carried inside the `Unknown` variant it returns.

Note the safety this buys, and preserve it: because the proof has no constructor, a caller cannot
hand-build a `CleanupDeadlineTransferV1::Unknown` containing one. The variant's own fields may stay
public; the proof's unconstructibility is what closes the hole.

**2 — Mint only where this call observed the condition itself.**

Decide, per branch, whether it mints — and justify each decision in the handoff:

- Branch 3 (**adopted durable terminal**) must **not** mint. Provenance is unknown by construction.
- Branch 1 (**foreign guard**) must **not** mint. A caller-chosen wrong guard says nothing about
  ownership, and minting there is the round-2 forgery.
- Branch 2 (**guard token not held**) is the candidate that plausibly mints: this call read live state
  under its own lock. If you conclude it should not mint either, say why, and say what would.

If **no** branch qualifies, stop and report rather than inventing one — that is a real finding, not a
failure.

**3 — Prove the non-minting branches yield no proof.**

Each non-minting branch needs its own test showing the returned `Unknown` carries no proof. These
negatives are the deliverable as much as the positive is — that is the lesson of 4E, and of the three
rounds this task replaces.

**4 — Prove unforgeability by compilation.**

A `trybuild` compile-fail case must show `UnidentifiableCleanupOwnerProofV1` cannot be constructed:
no `default`, no struct literal, no `From`. Follow the existing convention —
`crates/bridge-core/tests/compile_fail.rs` and the `tests/trybuild/` cases. Generate the `.stderr`
with `TRYBUILD=overwrite`, then verify a clean run.

**Do not add a `pub` field, a builder, a `#[doc(hidden)]` constructor, or a test-only constructor
reachable from another crate.** Any of those re-opens the class this task exists to close.

## Invariants — must not change

- `crates/bridge-workflow/src/executor.rs` is **byte-identical** to the base. Publish its SHA-256 on
  both trees in the handoff.
- No timer arms; no `select!`, sleep, spawn, token, or cancellation is added or altered.
- Existing `transfer_cleanup_deadline` behaviour is otherwise unchanged: the same branches are taken,
  the same results returned. This task adds a minted proof to one branch; it does not re-route control
  flow.
- Readiness ships `Disarmed`; `AutomaticR2f1b` stays unreachable from production.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched. If a change is
  genuinely unavoidable, **stop and report**.

**The refusal gate.** Re-assert, as 4B–4H-1 did, that no production caller can construct an automatic
attempt while readiness is `Disarmed`.

## Out of scope

- The `biased` select and any executor edit — **4H-2**.
- Wiring the ownerless observation into a live cleanup path.
- Changing `NodeCleanupObservationV1` or `NodeCleanupV2` encodings.

## Required tests

Each must fail on the pre-change tree — verify that, do not assume it:

1. The minting branch returns an `Unknown` carrying a proof, and the proof names the correct flight.
2. **Foreign guard** returns `Unknown` with **no** proof.
3. **Adopted durable terminal** returns `Unknown` with **no** proof — including when the durable record
   was pre-seeded through the public journal trait, which is the exact round-3 forgery.
4. Two ordinary sequential `transfer_cleanup_deadline` calls on the same flight do not yield a second,
   unearned proof — the round-3 route that needed no journal access.
5. `bridge-workflow` can construct `UnsettledUnknownOwnerless` **only** from a minted proof.
6. Compile-fail: the proof cannot be constructed by `default`, struct literal, or `From`.
7. The refusal gate.

## Size

**Cap: 250 counted lines** (added nonblank physical Rust lines after `cargo fmt`, docs excluded).
Projection: 150. The cap is a **stop boundary**. If the change cannot be made within it, **stop and
report**.

## Frozen single-mutation control

Produce a patch reverting exactly one **production** change, record its SHA-256, and verify it applies
cleanly, reddens at least one named test (report the **actual** red population from a **full-suite**
run as a set difference against the candidate's own failures), and that the mutated tree still passes
`cargo clippy --all-targets --all-features --locked -- -D warnings`.

Prefer **minting on the adopted-durable-terminal branch** — that is the exact forgery this task
closes, so a control that re-opens it is the strongest available.

If the container cannot fetch crates, use the warm cache offline —
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true` with localhost excluded from the injected proxy, and an
explicit `RUSTDOC`.

## Handoff

Write `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h1b-handoff.md` covering: **the per-branch mint
decision with a reason each**, the control patch path and SHA-256, the actual red population,
`executor.rs`'s SHA-256 on both trees, the deliberate exclusions, and the counted line total.

**Report gate results truthfully.** If the configured test command is not green, say so and name the
failing test. If a fixture was hand-written rather than tool-generated, say so. Exclude diagnostic runs
that failed for their own reasons, and name them.

**Note on the host suite:** nine `tests/smoke_cli.rs` / `tests/fallback_plan_cli.rs` failures are
environmental and intermittent on this lane. Report them; do not chase them into production changes.

**Do not record your own head or tree sha.**

## Acceptance Criteria

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`,
  `cargo build --locked`, and the configured test command are all green.
- Every test in "Required tests" exists and fails on the pre-change tree.
- `UnidentifiableCleanupOwnerProofV1` has no constructor of any kind reachable outside `bridge-core`.
- Both non-minting branches are proven to yield no proof.
- `executor.rs` is byte-identical to the base, proven by published SHA-256.
- Counted added nonblank Rust lines ≤ 250.

## Files

- `crates/bridge-core/src/retained_resource_flight.rs` — the minting branch.
- `crates/bridge-core/src/execution_policy.rs` — the proof type.
- `tests/trybuild/`, and test files under `bridge-core` and `bridge-workflow`.

## Spec Refs

- `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h1-operator-gate.md` — why the prior shape failed.
- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` — §5.5 cleanup ownership transfer.

## Commit Message

Mint the ownerless-cleanup proof at its point of observation (R2f1b slice 4H-1b)
