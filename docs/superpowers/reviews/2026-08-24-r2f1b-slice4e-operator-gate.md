# R2f1b slice 4E — operator gate and custody evidence

Implementation candidate: `76c1b2e9` ("Settle the closed impossibility-proof list and its negatives")
Base:                     `1685aa6c` (`origin/main`, R2f1b slice 4D)
Implementor:              `gpt-5.6-sol`, effort `xhigh`, review depth `thorough`
Loop:                     3 attempts — **converged**, `verify: PASS`, `review: APPROVE`

## Why the negatives were the deliverable

The decomposition sequences 4E ahead of the lower-impact warning behaviour because **a false positive
here cancels real work**. The spec therefore made the six named non-proofs first-class, each owed its
own test, with ambiguity resolving to "not proved" by rule. Delivered:

```
unknown_child_state_is_not_proof            no_output_is_not_proof
elapsed_silence_is_not_proof                file_mtime_is_not_proof
process_age_is_not_proof                    provider_slowness_is_not_proof
pid_reuse_or_undetermined_errno_is_not_child_exit_proof
ambiguous_container_removal_is_not_absence_proof
mismatched_container_id_is_not_absence_proof
one_open_route_prevents_all_routes_closed_proof
present_or_unknown_terminal_result_prevents_all_routes_closed_proof
```

Fourteen tests in all, against three admissible proofs.

## Invariants — checked against the tree, not the handoff

| Invariant | Result |
|---|---|
| `crates/bridge-workflow/src/executor.rs` untouched | **byte-identical**; the #22 bare await intact |
| `Cargo.lock` / manifests untouched | no changes |
| No production caller cancels on a proof | the proof is produced, never consumed |
| Arbitration readiness struct unmodified | `scheduler_arbitration.rs` untouched |

## Operator gate — candidate `76c1b2e9`, idle machine

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
| `76c1b2e9` (candidate) | 101 | 9 |
| `1685aa6c` (base) | 101 | 9 |

Set difference empty in both directions. The 9 are the host container/smoke tests in
`tests/smoke_cli.rs` and `tests/fallback_plan_cli.rs`.

**These nine came back.** They were present for 4A and 4B, **absent for 4C and 4D**, and are present
again here — on the base as well as the candidate. The population is environmental and intermittent,
which is the concrete vindication of 4C's note: **re-measure the base every slice**. A remembered
number would have been wrong in both directions within four slices.

## Frozen single-mutation control — independently re-measured

- Path: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4e-mutation-control.patch`
- SHA-256: `222dd10a7df360e4a319605a1e0de3fc90773cae46dc7e0a324ed2710734aeed` — matches the handoff
- Mutation: deletes exactly one conjunct from `retained_child_exit_is_unambiguous` —
  `signal.expected_start_time_ticks == immutable_start.start_time_ticks`. Production only.
- `git apply --check` clean; applied and reverted with the tree returning to `dirty=0`.

Newly red, computed as a set difference over a full-suite run (9 → 10):

```
mechanical_impossibility::tests::pid_reuse_or_undetermined_errno_is_not_child_exit_proof
```

The mutated tree passes `clippy -D warnings` with exit 0.

This is the control the spec asked for, and it lands exactly where intended: removing the start-time
check **is** the PID-reuse failure mode, and the one test that exists to catch it is the one that goes
red. A single reddened test is narrow, but here the narrowness is the point — the control names one
defect and one guard.

## Size

- Counted added nonblank Rust lines, `1685aa6c..76c1b2e9`: **382**
- Cap: **400**. Projection was 280.

## Residuals carried, not solved

1. **The sealing compile-fail fixture will churn in 4H, and can silently lose its meaning.** The
   expected output is ten errors, of which **seven are `E0603` "is private"** — the imports fail
   because the module is `pub(crate)`. Only three errors are the actual unconstructibility proof:
   `E0599` (no `default`), a plain `error` (struct literal, private fields), and `E0277`
   (`From<bool>` unsatisfied). Those three are present and precise, so the property holds today.

   But **4H must widen visibility to wire the adapters in**. When it does, the seven `E0603` errors
   disappear, the `.stderr` stops matching, the test goes red, and the obvious fix is to regenerate the
   fixture — at which point nothing checks whether the sealing errors survived.

   **Instruction for 4H:** after widening visibility, regenerate the fixture and confirm `E0599`, the
   private-fields error, and `E0277` are all still present. If they are not, the unconstructibility
   proof has been lost in regeneration, and the fixture must be rebuilt to assert it directly.

2. The adapters are `pub(crate)`, so 4H must widen visibility before wiring — the task deferred that
   deliberately (raised by the reviewers; the operator concurs).
3. Carried from 4D: post-cutoff completion drop is spec-silent and untested; 4H owes it a test or an
   explicit decision.
4. Carried from 4C: `UnidentifiableCleanupOwnerProofV1` is sealed with no public constructor outside
   `bridge-core`; `into_disposition`'s pending path is reconstruct-per-poll.
