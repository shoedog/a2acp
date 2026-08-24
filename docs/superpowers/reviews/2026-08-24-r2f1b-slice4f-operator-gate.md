# R2f1b slice 4F — operator gate and custody evidence

Implementation candidate: `60bb9f3f` ("Build the gated one-shot fixed-grace timer")
Base:                     `63134836` (`origin/main`, R2f1b slice 4E)
Implementor:              `gpt-5.6-sol`, effort `xhigh`, review depth `thorough`
Loop:                     2 attempts — **converged**, `verify: PASS`, `review: APPROVE`

## An operator spec defect, and the behaviour change it produced

**Read this before the gate tables.** The task said: *"Refuse `FixedGrace` when the frozen
`deadline_activation` is `ManualOnlyR2f1a`, exactly as today."* Those two halves cannot both hold.
Today's refusal keys on `PolicyActivationV1::Production`; keying on `ManualOnlyR2f1a` necessarily also
catches `PolicyActivationV1::ManualTest`, which maps to the same frozen activation.

The implementor followed the **rule** rather than the "as today" clause — the better of two
irreconcilable readings — and the reviewers flagged the discrepancy as MINOR. The delivered change:

```rust
-        if matches!(activation, PolicyActivationV1::Production) {
+        if matches!(controls.deadline_activation, DeadlineActivationV2::ManualOnlyR2f1a) {
```

**Consequences, stated plainly:**

| Case | Before | After |
|---|---|---|
| `(Disarmed, Production)` | refuse | refuse — **production identical** |
| `(Armed, Production)` | refuse | admit — the gated lift, unreachable while readiness is `Disarmed` |
| `ManualTest` | **admit** | **refuse** — a real behaviour change |

That last row required **flipping a pre-existing R2f1a test assertion**:

```rust
-    assert!(resolve_execution_policy_v1(&fixed, …, PolicyActivationV1::ManualTest).is_ok());
+    assert_eq!(resolve_execution_policy_v1(&fixed, …, PolicyActivationV1::ManualTest),
+               Err(ExecutionPolicyError::FixedGraceInactive));
```

**Operator assessment: accepted, and arguably more correct.** `ManualTest` means the manual R2f1a
contract, under which fixed grace *is* inactive; R2f1b lifts the refusal **only** under
`AutomaticR2f1b`. The `InvalidFixedGrace` bounds stay reachable via `(Armed, Production)`, so nothing
became untestable. But this is zero **production** blast radius, not zero blast radius — an inverted
R2f1a assertion belongs in the evidence, not buried in a diff. This is the **ninth** operator contract
defect of the same family in this lane: a requirement whose two halves contradict each other.

## What the base measurement had already predicted

The spec was written knowing the `InvalidFixedGrace` bounds check sits **after** the refusal, so under
`Production` it was unreachable — and therefore untested on that path. Lifting the refusal makes it
reachable for the first time, which is why the task required both the zero and over-cutoff cases
explicitly. Both are delivered.

## Invariants — checked against the tree, not the handoff

| Invariant | Result |
|---|---|
| `crates/bridge-workflow/src/executor.rs` untouched | **byte-identical**; the #22 bare await intact |
| `Cargo.lock` / manifests untouched | no changes |
| Production behaviour identical to base | `(Disarmed, Production)` still refuses |
| No timer armed from production | the timer is constructed and driven only in tests |

## Operator gate — candidate `60bb9f3f`, idle machine

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
| `60bb9f3f` (candidate) | 101 | 9 |
| `63134836` (base) | 101 | 9 |

Set difference empty in both directions. The 9 are the intermittent host container/smoke tests —
present for 4A, 4B, 4E and now 4F; absent for 4C and 4D.

## Frozen single-mutation control — independently re-measured

- Path: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4f-mutation-control.patch`
- SHA-256: `bbae605d6e0955612d22ff613b631bbcd42c0e01c751d342a70993f28a44f7d8` — matches the handoff
- Mutation: **makes the timer renewable** — the arm guard changes from "not `Unarmed`" to "is
  `Armed`", so a *fired* timer can be re-armed. Production only.
- `git apply --check` clean; applied and reverted with the tree returning to `dirty=0`.

Newly red, computed as a set difference over a full-suite run (9 → 10):

```
timer_arms_once_and_fires_once_without_renewal
```

The mutated tree passes `clippy -D warnings` with exit 0.

One-shot-ness is this sub-slice's entire safety property, and the control breaks exactly it. A single
reddened test, precisely aimed.

## Size

- Counted added nonblank Rust lines, `63134836..60bb9f3f`: **300**
- Cap: **300**. Projection was 200.
- **Exactly at the boundary** — the tightest fit in the programme. Inside the cap, but with zero
  headroom; a follow-up fix round here would have breached it.

## Residuals carried, not solved

1. Carried from 4E and **load-bearing for 4H**: the sealing compile-fail fixture expects ten errors of
   which seven are `E0603` "is private". 4H must widen visibility to wire the adapters, which deletes
   those seven and breaks the fixture. After regenerating it, 4H must confirm `E0599`, the
   private-fields error, and `E0277` all survive — otherwise the unconstructibility proof is silently
   lost.
2. Carried from 4E: the impossibility adapters are `pub(crate)`; 4H must widen visibility.
3. Carried from 4D: post-cutoff completion drop is spec-silent and untested; 4H owes it a test or an
   explicit decision.
4. Carried from 4C: `UnidentifiableCleanupOwnerProofV1` is sealed with no public constructor outside
   `bridge-core`; `into_disposition`'s pending path is reconstruct-per-poll.
