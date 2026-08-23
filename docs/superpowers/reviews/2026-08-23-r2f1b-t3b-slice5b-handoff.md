# T3b slice 5B — readiness handoff

## Scope

Base: `origin/main` = `d6b3bb4d`.

This slice flips the sole remaining exact-absence production readiness gate. The report continues to provide ordered historical evidence, never settlement authority.

## Changed files

- `crates/bridge-worktree/src/sweep/report.rs`
- `crates/bridge-worktree/src/sweep.rs`
- `crates/bridge-worktree/src/settle.rs`
- `docs/superpowers/reviews/2026-08-23-r2f1b-t3b-slice5b-readiness-control.patch`
- `docs/superpowers/reviews/2026-08-23-r2f1b-t3b-slice5b-handoff.md`

`Cargo.lock` and every manifest are unchanged.

## Counted Rust lines

19 added nonblank physical Rust lines after formatting, against the cap of 200.

## Frozen control

Control: `docs/superpowers/reviews/2026-08-23-r2f1b-t3b-slice5b-readiness-control.patch`

SHA-256: `a35e2d0ce7e617c37e3724034c8a187adb59426643589f0ef42548bf5aae2de2`

The single mutation makes the action-time target-observation fixture treat a recreated directory as absent (`is_file` rather than `exists`), accepting historical absence without a current target re-proof. It applies cleanly. Across the full bridge-worktree library suite with the control applied, exactly `settle::tests::readiness_true_still_refuses_a_stale_entry` is red (362 passed, 1 failed); after reversal, the full suite passes.

## Arming point

This commit is the arming point for the exact-absence settlement subsystem. Reverting it disarms the subsystem completely while preserving the action-time re-prove obligation and stranded-marker rules.

## Operator gates

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**
