# R2f1b slice 4B — operator gate and custody evidence

Implementation candidate: `10b1700b` ("Make AutomaticR2f1b constructible and fully refused")
Base:                     `0ef9b58c` (`origin/main`, R2f1b slice 4A)
Implementor:              `gpt-5.6-sol`, effort `xhigh`, review depth `thorough`
Loop:                     2 attempts — **converged** within the declared bound of 3

## Review loop

| Round | Verdict | Finding |
|---|---|---|
| 1 | REJECT | The frozen mutation control's evidence was never **measured** — `cargo test` and `cargo clippy` on the mutated tree both exited before compiling (crates.io proxy 403 on `a2a-lf`). Disclosed honestly; no red population claimed. |
| 2 | **APPROVE** | Control measured. Top remaining issue a MINOR line-count discrepancy (343 vs 342, both under cap). |

Round 1's rejection was correct and worth recording: Reviewer B had static-traced the mutation to four
plausibly-reddened tests, and the reviewers refused to accept that trace as a substitute for a
measured run. **A probe that fails for its own reasons yields no evidence.** The static trace also
turned out to be an undercount — the real population is seven.

The operator predicted a retry would fail identically, on the theory that the editing container
cannot reach crates.io. **That prediction was wrong.** The implementor found the route:
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true` against the warm dependency cache, with localhost excluded
from the injected proxy so loopback fixtures stayed local.

## Frozen single-mutation control — independently re-measured by the operator

- Path: `docs/superpowers/reviews/2026-08-23-r2f1b-slice4b-mutation-control.patch`
- SHA-256: `2bae9460b7eccb13f861d616ead56f756d112dbe9329b7097b7abba3b934e5a7` — matches the handoff
- Mutation: the sole automatic arm of `deadline_activation_v2_for`, `(Armed, Production)` →
  `(Disarmed, Production)`. **Production only**, not a test fixture.
- `git apply --check` clean; applied and reverted with the tree returning to `dirty=0`.

Measured on the host over a **full** workspace run, computed as a set difference against the
candidate's own pre-existing failures rather than by reading a summary line:

| | Distinct failures |
|---|---|
| Candidate | 9 |
| Mutated | 16 |
| **Newly red under the mutation** | **7** |

```
admission::slice4b_tests::automatic_v3_refuses_legacy_watchdog_before_effects
disarmed_production_admission_never_obtains_automatic_activation
graph::tests::workload_fingerprint_partitions_deadline_activation_without_moving_manual_baseline
manual_activation_admits_legacy_watchdog_configuration
manual_v3_identity_is_the_unbound_historical_fingerprint
manual_workload_fingerprint_matches_the_pinned_golden
scheduler_readiness_and_policy_activation_select_deadline_activation
```

Seven names, matching the handoff's seven exactly — an independent reproduction, not a re-read of the
implementor's claim.

**The mutated tree survives `cargo clippy --workspace --all-targets --all-features --locked -D warnings`
with exit 0.** This is the check that voided a control earlier in this lane: a control failing on
`dead_code` before reaching its red tests proves nothing.

## Operator gate — candidate `10b1700b`, idle machine

| Gate | Exit |
|---|---|
| `cargo fmt --all -- --check` | 0 |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 |
| `cargo test -p bridge-core --locked --no-fail-fast` | 0 |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` | 0 |
| `cargo test --workspace --locked --no-fail-fast` | 101 — 9 distinct failures |

## Attribution control — same environment, same machine, sequential

| Tree | Workspace exit | Distinct failures |
|---|---|---|
| `10b1700b` (candidate) | 101 | 9 |
| `0ef9b58c` (base) | 101 | 9 |

Set difference empty in **both** directions: no regression, no incidental fix. The 9 are the host
container/smoke tests pre-existing on `main` (`tests/smoke_cli.rs`, `tests/fallback_plan_cli.rs`).

Runs were sequential, never concurrent — this suite has inflated from 11 to 29 failures under
parallel load on an identical tree.

## Size

- Counted added nonblank Rust lines, `0ef9b58c..10b1700b`: **342**
- Cap: **350**. Projection was 250.
- The handoff records 343; the operator measures 342. Both are under cap and the discrepancy changes
  nothing, but the operator count is the one bound to this tree.

## Scope confirmations

- Readiness ships **`Disarmed`**. `AutomaticR2f1b` is unreachable from any production caller.
- No timer arms; `crates/bridge-workflow/src/executor.rs` is untouched.
- `FixedGraceInactive` still fires for `FanOutPolicyV1::FixedGrace` under `Production`.
- Manual-activation wire bytes are unchanged, asserted against literal bytes rather than a round-trip.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` untouched.

## Residuals carried, not solved

1. **`contract_fingerprint` is not yet in the workload fingerprint.** The scope document (§2.1) pairs it
   with activation, but `FrozenR2f1bContractV1::with_computed_fingerprint` still has zero production
   callers, so there is nothing to incorporate. Owed by the sub-slice that first constructs the contract.
2. **"Fully refused" is an admission-layer property, not a construction-layer one.**
   `resolve_execution_policy_with_readiness_v1` is `pub` behind `#[doc(hidden)]`, and the watchdog
   refusal lives in `admission.rs`. A future in-workspace caller could import the function, pass
   `Armed`, and bypass the watchdog check. 4C–4J must keep admission as its only production entry point.
3. **`DeadlineActivationV1` is now fully orphaned** — zero references outside its own declaration.
   Cosmetic; still `pub`, so clippy stays clean.
