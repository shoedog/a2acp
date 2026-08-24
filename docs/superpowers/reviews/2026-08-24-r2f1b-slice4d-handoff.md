# R2f1b slice 4D handoff: scheduler arbitration kernel

## What changed

- Added `bridge_workflow::scheduler_arbitration`, a pure synchronous and clock-free arbitration seam with one executable eight-arm priority array:
  1. ready node completions;
  2. durable trigger-barrier acknowledgements;
  3. workflow or external cancellation;
  4. fixed grace expiry;
  5. absolute cutoff;
  6. mechanically proved impossibility;
  7. due no-progress snapshots;
  8. wait for node activity, control, or clock.
- Added caller-supplied readiness and tie facts. Arbitration samples no clock and performs no wait, cancellation, persistence, or other effect.
- Made the exact-cutoff rule inclusive: completions with `ready_at_ms == absolute_cutoff_at_ms` drain first, then the result identifies unfinished in-flight nodes for later cancellation. Ready completion batches and the follow-up cancellation list are sorted by `NodeId`.
- Added one explicit hand-authored readiness/expected-winner table. It covers every arm, the wait fallback, and an all-eight-ready row that expects arm 1. The table imports no production priority representation and computes no ordering. All test fixtures and expectations are hand-written; no fixture generator was used.
- Added direct tie tests for inclusive cutoff completion, post-drain unfinished-node cancellation, warning loss to completion and cutoff, and sorted completion batching. A direct adjacent-arm test strengthens the frozen mutation control.

## Red-first evidence

Before the production module and export existed, the exact focused target

```text
cargo test -p bridge-workflow --test r2f1b_slice4d_scheduler_arbitration --locked --no-run
```

exited 101 because `bridge_workflow::scheduler_arbitration` was unresolved. Thus the pre-change tree could not compile the required test target; its named tests did not execute individually. After the seam was added, the target passes all 8 tests.

## Frozen mutation control

- Patch: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4d-mutation-control.patch`
- SHA-256: `558057022e9ba2abc61d423a893945a767928c51bdb932e7d5d78a13b795de7f`
- Mutation: swap adjacent production arms 1 and 2, placing durable barrier acknowledgement ahead of ready completion drain.
- The patch applied cleanly to the candidate and, after reversal, still passes `git apply --check`.
- The candidate full-suite failure set was empty. The mutated full-suite run failed only the `r2f1b_slice4d_scheduler_arbitration` target. Its actual red set difference was:
  - `completion_outprioritizes_durable_barrier_acknowledgement`
  - `scheduler_priority_table_is_exhaustive_and_all_ready_selects_arm_one`
- The mutated tree passed `cargo clippy --all-targets --all-features --locked -- -D warnings`.
- The patch was reversed after the control; the focused candidate target then returned to 8/8 green.

The candidate and mutant full-suite commands used `CARGO_HOME=/cargo`, `CARGO_NET_OFFLINE=true`, `CARGO_INCREMENTAL=0`, an explicit installed `RUSTDOC`, and `NO_PROXY=no_proxy=localhost,127.0.0.1`. The localhost exclusion is necessary in this environment because HTTP(S) proxy variables are injected and the suite uses local fixture servers.

## Verification

- `cargo fmt --all -- --check`: green.
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: green on the candidate and the mutation.
- `cargo build --locked`: green.
- Focused scheduler target: 8 passed, 0 failed.
- Configured workspace command (`--exclude bridge-container` and the three configured process-test skips): green on the candidate, including doc tests, with the environment described above.
- Full-suite mutation control using that same configured command: expected red, with exactly the two named scheduler failures above and no candidate pre-existing failures.

Diagnostic runs made before honoring both environment requirements are not gate evidence: a run without explicit `RUSTDOC` could not launch doc tests, and runs without the localhost proxy exclusion routed local fixture traffic through the injected egress proxy and produced unrelated transport/timing failures. An earlier concurrent candidate pass also observed one transient compatibility process-status failure; the isolated test passed 10/10 immediately afterward, and the correctly configured final full suite was green.

## Deliberate exclusions and stop boundaries

- `executor.rs` is unchanged. The old unguarded `inflight.next().await` path remains untouched; this slice does not wire or activate the kernel.
- Production automatic readiness remains disarmed and production admission remains manual-only R2f1a. The new refusal test checks both facts.
- No timer owner, scheduler loop, `select!`, sleep, spawn, cancellation token, cancellation effect, executor transition, or persistence effect was added.
- The fixed grace constant/policy was not changed.
- Mechanical impossibility is only a caller-supplied readiness fact here; proof construction belongs to its later slice.
- The cancellation list is a pure post-winner plan, not an executed cancellation.
- No work from slices 4E through 4I is included.

## Counted size

The added Rust totals **388 nonblank physical lines after `cargo fmt`**: 387 across the new module and test target plus the one-line module export. The 450-line stop boundary therefore has 62 lines of headroom. Documentation and the frozen patch are excluded from the count as specified.
