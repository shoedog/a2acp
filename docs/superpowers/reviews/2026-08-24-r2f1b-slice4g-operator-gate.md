# R2f1b slice 4G — operator gate and custody evidence

Implementation candidate: `7f6c419c` ("Settle progress epochs and no-progress warning cadence")
Base:                     `23e331c6` (`origin/main`, R2f1b slice 4F)
Implementor:              `gpt-5.6-sol`, effort `xhigh`, review depth `thorough`
Loop:                     3 attempts — **converged**, `verify: PASS`, `review: APPROVE`

## The review loop earned its keep here

| Round | Verdict | Finding |
|---|---|---|
| 1 | REJECT | Gate red on `bridge-store --lib`, and the handoff claimed *"failures: none"* — contradicting its own evidence. No base-tree control run. |
| 2 | REJECT | **BLOCKER:** `UsageHighWater` and `OwnedChildOutput` classified as *meaningful progress*, letting a chatty-but-stuck attempt suppress its warnings indefinitely. |
| 3 | **APPROVE** | Classification corrected; all eight required tests and the safety invariant independently verified. |

Round 2 is the one worth noting. The task stated an explicit **default-to-not-progress asymmetry** with
its reasoning: warnings never cancel, so a spurious warning is harmless while a wrongly-suppressed one
hides a stuck node. The reviewers checked the delivered classification **against that stated principle**
and found two variants on the wrong side. A spec that had merely said "classify progress sensibly"
would have given them nothing to check, and both would have passed as plausible.

The `bridge-store` failure from round 1 (`ExecutableFileBusy`, "Text file busy" — a
write-while-executing artifact in a crate this diff never touches) did **not** recur in round 3's
verify. It was non-deterministic, and no operator control was needed to establish that.

## Invariants — checked against the tree, not the handoff

| Invariant | Result |
|---|---|
| `crates/bridge-workflow/src/executor.rs` untouched | **byte-identical**, independently confirmed: base and candidate both hash to `def9c4fc…684d4` |
| `Cargo.lock` / manifests untouched | no changes |
| Classification total and wildcard-free | one `match`, all thirteen variants named, no `_` arm |
| Silence never cancels | warning-only poll result whose cancellation, impossibility and terminal-effect queries are unconditionally false, including at `u64::MAX` |

The handoff volunteered the executor's SHA-256 as its byte-identity evidence rather than asserting
"untouched". The operator recomputed it on both trees; it matches.

## The classification, and one judgement left open

Correct on the three chatty signals:

```rust
UsageHighWater   => false      // corrected in round 3
OwnedChildOutput => false      // corrected in round 3
Heartbeat        => false
```

There is also a guard the task did not ask for and which strengthens the result: a progress-capable
reason only counts when its proposed high-water **strictly advances** for that `(phase, reason)` cell,
so replaying a status cannot reset the epoch.

**Open judgement — `MessageDelta` and `ThoughtDelta` are classified as progress.** The handoff
justifies this as "new producer-output bytes arrived", which is defensible. But it is also the
chatty-but-stuck case in its purest form: an agent looping while streaming genuinely *new* tokens
advances the byte high-water every time, resets its epoch indefinitely, and never warns. The
strict-advance guard does not help, because the new text really is new.

Not blocked — two reviewers approved, the rationale is explicit and reviewable exactly as the task
required, and the opposite choice would fire warnings during ordinary long generations. Recorded
because it is the residual most likely to matter in practice: **if no-progress warnings prove too
quiet in real runs, this is the first line to revisit.**

## Operator gate — candidate `7f6c419c`, idle machine

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
| `7f6c419c` (candidate) | 101 | 9 |
| `23e331c6` (base) | 101 | 9 |

Set difference empty in both directions. The 9 are the intermittent host container/smoke tests —
present for 4A, 4B, 4E, 4F and now 4G; absent for 4C and 4D.

## Frozen single-mutation control — independently re-measured

- Path: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4g-mutation-control.patch`
- SHA-256: `80408cb018aaa558838679225490550d986c1ba307e918e6539d16f0cee83978` — matches the handoff
- Mutation: **resets the progress epoch on ordinary activity as well as progress**. Production only.
- `git apply --check` clean; applied and reverted with the tree returning to `dirty=0`.

Newly red, computed as a set difference over a full-suite run (9 → 10):

```
non_progress_activity_updates_only_activity_clock_and_epoch_keeps_climbing
```

The mutated tree passes `clippy -D warnings` with exit 0.

This is the control the task asked for, and it walks into exactly the failure the cadence exists to
prevent: under the mutation, a chatty but stuck node suppresses its own warnings forever.

## Size

- Counted added nonblank Rust lines, `23e331c6..7f6c419c`: **344**
- Cap: **350**. Projection was 250.

## Residuals carried, not solved

1. **A delayed first poll skips ordinals.** Poll first at 65 minutes and ordinal 1 never emits.
   Raised as MAJOR by the reviewers; it breaches no required test and does not weaken
   silence-never-cancels, and polling cadence belongs to the wiring slice. **4H owns it.**
2. **`MessageDelta`/`ThoughtDelta` as progress** — the open judgement above.
3. Carried from 4E and load-bearing for 4H: the sealing compile-fail fixture expects ten errors of
   which seven are `E0603`. 4H must widen visibility, which deletes those seven and breaks the
   fixture; after regenerating it, 4H must confirm `E0599`, the private-fields error and `E0277` all
   survive.
4. Carried from 4E: the impossibility adapters are `pub(crate)`; 4H must widen visibility.
5. Carried from 4D: post-cutoff completion drop is spec-silent and untested; 4H owes it a test or an
   explicit decision.
6. Carried from 4C: `UnidentifiableCleanupOwnerProofV1` is sealed with no public constructor outside
   `bridge-core`; `into_disposition`'s pending path is reconstruct-per-poll.
