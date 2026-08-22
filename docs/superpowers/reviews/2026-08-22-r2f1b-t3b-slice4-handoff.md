# T3b slice 4 handoff — unused settlement

## Scope

- Base `origin/main`: `c343e563`.
- Changed implementation/test files: `bin/a2a-bridge/src/compatibility_resolution.rs`, `bin/a2a-bridge/src/compatibility_schedule_state.rs`, `bin/a2a-bridge/src/storage_report.rs`, `crates/bridge-worktree/src/custody_writer.rs`, and `crates/bridge-worktree/src/settle.rs`.
- Review artifacts: this handoff and `2026-08-22-r2f1b-t3b-slice4-mutation-control.patch`.
- Rust added-line count: 747 nonblank lines, below the 790-line cap.
- Cargo manifests and `Cargo.lock` are unchanged.

## Settlement boundary

`WorktreeCustodianV1::replace_unused_settled` opens the held `SettlementWindowV1`, re-proves the report-selected candidate under that same window, publishes only the frozen `ProtectionPrepared -> UnusedSettled` transition, and clears the claim. It uses the shared `publish_custody_record_in(pin, name, record, mode)` publication helper. An ambiguous replace is reported as `unused-settlement-transition-uncertain`, stops before retirement, and is never collapsed into a no-effect refusal. The checkout is never changed.

The durable marker is retired only with `retire_captured_regular_child_v2` after its descriptor-bound content snapshot. A post-transition interruption leaves an operator-visible `stranded-unused-settled` record; a captured retirement marker is separately reported by storage as `R2f1b retired custody marker residue (captured; recovery-owned)`. The new terminal state has no outgoing transition and no source field or claim.

Driving integration remains intentionally deferred: a later sweep/backend slice must invoke this capability; this slice supplies the proof-bound production boundary only.

The verification companion gives the descriptor-stress archive fixture 120 seconds of scheduling headroom and releases its real test descriptor before injecting an unlock-report failure, preventing unrelated parallel test scheduling or an expected panic from producing a false tree-drift or retained-lock failure.

## Bounded-effect audit

The colocated audit starts above the settlement outcome/helper block and source-pins the full settlement and publication boundaries with named missing-anchor diagnostics. It rejects direct process starts, direct filesystem deletion or rename, and Git mutation argv construction. The production probe is `HostGitWorktree`; its exact reachable operation inventory is `git rev-parse` and `list_porcelain_argv` (`git worktree list --porcelain -z`), while the known mutating `remove_argv`, `prune_argv`, and `add_argv` builders are forbidden.

Focused validation passed:

- `cargo fmt --all -- --check`
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo clippy -p bridge-worktree --all-targets --locked -- -D warnings`
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --lib --locked --no-fail-fast` (356 library tests passed)
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test -p a2a-bridge storage_report::tests::a_captured_custody_marker_is_reported_as_distinct_evidence -- --exact`
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test -p a2a-bridge --bin a2a-bridge --locked --no-fail-fast` (1,095 tests passed)
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo clippy -p a2a-bridge --bin a2a-bridge --locked -- -D warnings`
- The required workspace and package-level operator gates remain pending below.

## Mutation control

- Control: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice4-mutation-control.patch`
- SHA-256: `cb7667c947558e2d6fb041c565a9aa419ac0be8392db107e0e3226d817aeac3f`
- Logical mutation: pass a wrong child name to the descriptor-safe marker-retirement primitive.
- Applying the control produces the single expected red test: `settle::tests::unused_candidate_settles_only_after_exact_absence`; the control was then reversed and the clean focused suite passed.

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**
