# R2f1b slice 4H-1b — operator gate and custody evidence

Implementation candidate: `f9edf711` ("Mint the ownerless-cleanup proof at its point of observation")
Base:                     `9ade2c49` (`origin/main`, R2f1b slice 4H-1)
Implementor:              `gpt-5.6-sol`, effort `xhigh`, review depth `thorough`
Loop:                     **1 attempt — converged**, `verify: PASS`, `review: APPROVE`, **no forgery
                          route found by either reviewer**

## The contract was the defect — and changing it fixed the slice in one round

Same requirement, same reviewers, same implementor, same crate.

| Contract shape | Rounds | Outcome |
|---|---|---|
| **Validate** a caller-supplied `CleanupDeadlineTransferV1` (4H-1 item 5) | 3 | Three *distinct* forgery routes; withdrawn as open-class |
| **Mint** at the point of observation (this slice) | **1** | Approved; both reviewers hunted for a forgery route and found none |

The delivered shape:

```rust
pub(crate) fn mint_at_ownerless_observation(resource_flight_id: ResourceFlightIdV1) -> Self
```

`pub(crate)`, so `bridge-workflow` cannot mint. It can only *receive* a proof inside the `Unknown`
variant the call returns. And because the proof has no cross-crate constructor, a caller cannot
hand-build a `CleanupDeadlineTransferV1::Unknown` containing one — the variant's public fields are
harmless; the proof's unconstructibility is the entire lock.

**Per-branch outcome — four `proof: None`, exactly one `proof: Some(..)`:**

| Branch | Mints? | Why |
|---|---|---|
| Foreign guard (`!Arc::ptr_eq`) | **No** | Caller passed another flight's guard — caller error, not an observation. This was **round 2's forgery**. |
| Guard token not held, under this call's own lock | **Yes** | This call established the condition against live state it owns. |
| Adopted durable terminal | **No** | Provenance unknown by construction; the record may have been journaled by anything. This was **round 3's forgery**. |

## The control re-opens exactly the hole this slice closed

- Path: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h1b-mutation-control.patch`
- SHA-256: `c18ac3de412a90c37e033a5401f8884df824cdbd5e91529a28e6625c0327a5d8` — matches the handoff
- Mutation: mints on the **adopted-durable-terminal** branch. Production only.
- `git apply --check` clean; applied and reverted with the tree returning to `dirty=0`.

Newly red, as a set difference over a full-suite run (9 → 11):

```
retained_resource_flight::tests::cleanup_deadline_public_journal_preseed_carries_no_ownerless_proof
retained_resource_flight::tests::sequential_cleanup_deadline_transfers_do_not_mint_a_second_proof
```

Both of round 3's routes go red together — the journal pre-seed *and* the two-sequential-calls route
that needed no journal access at all. The mutated tree passes `clippy -D warnings` with exit 0.

This is the strongest control in the programme: it does not merely break a test, it **reconstructs the
exact defect three review rounds spent themselves on**, and the two tests written to catch it both
fire.

## Operator gate — candidate `f9edf711`, idle machine

| Gate | Exit |
|---|---|
| `cargo fmt --all -- --check` | 0 |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 |
| `cargo test -p bridge-core --locked --no-fail-fast` | 0 |
| `cargo test -p bridge-workflow --locked --no-fail-fast` | 0 |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` | 0 |
| `cargo test --workspace --locked --no-fail-fast` | 101 — 9 distinct failures |

## Attribution control — same environment, same machine, sequential

| Tree | Workspace exit | Distinct failures |
|---|---|---|
| `f9edf711` (candidate) | 101 | 9 |
| `9ade2c49` (base) | 101 | 9 |

Set difference empty in both directions.

## A review finding the operator does not accept as stated

The reviewers' sole issue was *"a stale 'Not green' gate entry in the handoff that contradicts the
actual passing final verify run."* **It is not a contradiction — the two ran different commands.**

The bridge's configured verify carries `--skip staged_candidate_`. The handoff ran the workspace suite
**without** that skip and hit `staged_candidate_exec_is_bound_to_the_verified_file_object`, a known
intermittent process fixture the config excludes deliberately (added 2026-08-09 in the 2c2 fold,
precisely so the fix loop would not chase environment-red tests into production surgery).

The handoff named the failing test, identified it as intermittent, and stated it was not chased into
production changes. That is **more** conservative than the contract requires, not less. Recording the
correction so the note does not read as sloppiness in a later audit.

## Frozen invariants

| Invariant | Result |
|---|---|
| `executor.rs` byte-identical | `def9c4fc…684d4` on both trees — handoff published it, operator recomputed |
| `Cargo.lock` unchanged | `56a948ba…af86` on both trees |
| Proof constructor reachable outside `bridge-core` | **none** |

## Size

- Counted added nonblank Rust lines, `9ade2c49..f9edf711`: **150**
- Cap: **250**. Projection was 150 — exact.

## The 4H-2 dependency is now satisfied

`bridge-workflow` can obtain a minted `UnidentifiableCleanupOwnerProofV1` and therefore construct the
ownerless-`Unknown` observation. The sequencing constraint recorded in 4H-1's evidence is discharged;
**4H-2 is unblocked.**

## Residuals carried

1. From 4C: `into_disposition`'s pending path is reconstruct-per-poll, not mutate-or-reuse — flagged
   for the 4H-2 author.
2. From 4G, an open judgement for the owner: `MessageDelta`/`ThoughtDelta` count as meaningful
   progress. Defensible, but it is the chatty-but-stuck case; first line to revisit if no-progress
   warnings prove too quiet in practice.
