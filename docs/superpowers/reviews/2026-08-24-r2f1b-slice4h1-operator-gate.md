# R2f1b slice 4H-1 — operator gate and custody evidence

Implementation candidate: `04b5c095` ("Discharge the 4H wiring residuals without touching the executor")
Base:                     `7d2fb43b` (`origin/main`, R2f1b slice 4G)
Implementor:              `gpt-5.6-sol`, effort `xhigh`, review depth `thorough`
Loop:                     3 attempts — **bound reached, REJECT** — then item 5 withdrawn and a
                          decoupled fix turn applied

## Two planning decisions, both taken before the work failed rather than after

**1. 4H was split before dispatch.** The decomposition declares one 4H at a 500-line cap. It arrived
carrying **five** residuals — from 4C, 4D, 4E and 4G — on top of its own integration work, which its
350-line projection never assumed. Slice 4C was the comparable case: also a 500-cap slice with a rich
obligation set, and it consumed all three loop attempts plus two decoupled fix turns. So 4H was split
into 4H-1 (discharge the obligations, executor untouched) and 4H-2 (the single risky edit).

**2. Item 5 was withdrawn mid-slice — the requirement was wrong, not the implementation.**

| Round | Forgery route found |
|---|---|
| 1 | Public fields of `CleanupDeadlineTransferV1::Unknown` |
| 2 | Foreign-guard branch minting from a caller-chosen wrong guard |
| 3 | A durably-journaled `Settled{Unknown, recovery_owner: None}` row — reachable by two ordinary sequential `transfer_cleanup_deadline` calls, **or** by pre-seeding through the public journal trait |

Three rounds, three distinct routes, one requirement: **open-class**, and the discipline says park and
escalate rather than spend a fourth attempt. The diagnosis is the operator's own:
`from_cleanup_deadline_transfer_v1(&CleanupDeadlineTransferV1)` is a *validate-a-caller-supplied-value*
shape, and any such shape is forgeable when the caller can obtain the value. "Evidence that ownership
is genuinely unidentifiable" is a claim about **provenance**, and provenance cannot be established by
inspecting a value.

Owner approved the scope change. The replacement will use a **minting** contract —
`transfer_cleanup_deadline` returning the proof from the branch that observes the condition, with no
public constructor to forge.

**Item 5 was two-thirds of the slice**: counted lines fell from **290 to 108** on its removal.

The withdrawal was surgical, verified against the base rather than taken on report:

| Check | Result |
|---|---|
| `impl UnidentifiableCleanupOwnerProofV1` blocks | **0** — identical to base, no public constructor |
| `resource_flight.rs`, `retained_resource_flight.rs` vs base | **empty diffstat** — fully reverted, no collateral damage |
| `executor.rs` | byte-identical, `def9c4fc…684d4` on both trees |

## The residual that mattered most — discharged, and independently checked

4E's sealing fixture was the load-bearing item: widening visibility deletes seven `E0603` "is private"
errors, breaking it, and a careless regeneration would leave a green test proving nothing.

Operator census of the regenerated `.stderr` against the base:

| Error | Base | Candidate |
|---|---|---|
| `E0603` (visibility noise) | 7 | **0** — expected, visibility widened |
| `E0599` no `default` | 1 | **1** |
| plain — struct literal, private fields | 1 | **1** |
| `E0277` `From<bool>` unsatisfied | 1 | **1** |

All three sealing diagnostics survived. The unconstructibility proof is intact.

## The two deferred decisions, now made and tested

**Post-cutoff completion (4D residual).** A completion at the **exact** cutoff stays inclusive and
drains. A completion strictly **after** the cutoff is excluded, recorded in a deterministically sorted
`post_cutoff_completions` audit field, and its node stays in `nodes_to_cancel_after_winner`. Reason
given: the node was unfinished at the authoritative cutoff, and admitting a later completion would
make the boundary depend on polling delay — while silently dropping it would lose attribution
evidence. The audit field is a better answer than either of the two the task offered.

**Delayed first poll (4G residual).** A first poll at 65 minutes emits **one** warning at ordinal 2 and
records `superseded_ordinal_count = 1`; no ordinal-1 catch-up burst. Reason given: one warning per poll
keeps the path bounded and avoids turning a delayed observer into backpressure, while the ordinal plus
the superseded count preserve the cadence evidence.

## Operator gate — candidate `04b5c095`, idle machine

The fix turn stated plainly that it **could not run the Cargo gates** — empty registry cache, no
reachable container runtime — and separated source-valid checks from a dependency-resolution refusal
rather than reporting a pre-compile failure as a test result. Correct discipline, and it means this
gate is doing real verification rather than confirmation.

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
| `04b5c095` (candidate) | 101 | 9 |
| `7d2fb43b` (base) | 101 | 9 |

Set difference empty in both directions. The 9 are the intermittent host container/smoke tests.

## Frozen single-mutation control — re-cut, and re-measured

The original control targeted the withdrawn constructor, so it was re-cut against the surviving
post-cutoff decision.

- Path: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h1-mutation-control.patch`
- SHA-256: `932d35749f4babf6bfa632115891dd4deeaed6b3ef5414e10adfad3132cebae5` — matches the handoff
- `git apply --check` clean; applied and reverted with the tree returning to `dirty=0`.

Newly red, as a set difference over a full-suite run (9 → 10):

```
completion_strictly_after_cutoff_is_dropped_and_its_node_is_cancelled
```

The mutated tree passes `clippy -D warnings` with exit 0. The fix turn's red/green claim could not
have been executed in its environment; it reproduces here.

## Size

- Counted added nonblank Rust lines, `7d2fb43b..04b5c095`: **108**
- Cap: **300**.

## Sequencing constraint for 4H-2 — not an oversight

`UnidentifiableCleanupOwnerProofV1` still has **no public constructor**, so `bridge-workflow` cannot
construct the ownerless-`Unknown` observation. **Either the minting-contract slice lands before 4H-2,
or 4H-2 defers that path explicitly.** This is a stated dependency, recorded so the wiring slice does
not discover it mid-flight.

## Residuals carried

1. The ownerless-proof minting contract — withdrawn from here, owed its own slice.
2. Carried from 4C: `into_disposition`'s pending path is reconstruct-per-poll, not mutate-or-reuse.
3. Carried from 4G, open judgement for the owner: `MessageDelta`/`ThoughtDelta` count as progress.
