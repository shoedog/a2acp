# T3a increment 3A-2 handoff — typed claim authority

## Candidate checkpoint

- Implementation base: `b73e0a5a058eaad6d89a1be96d19a6ae4afe7e5c`.
- The implementation changes only `crates/bridge-worktree/src/sweep.rs`; `report.rs`, `host_git.rs`, `checked_scan.rs`, manifests, and `Cargo.lock` remain untouched.
- The formatted diff adds 353 nonblank physical Rust lines, within the 380-line cap.
- No Cargo build, test, clippy, or hygiene gate was run in this implementation container.

## Implementation

- `from_claim` and `from_bound` now return `ClaimAuthorityUnavailableV1`. Every constructor-era failure is mapped to the existing object/reason vocabulary, preserving `ObservationUnavailable` for failed observations and reserving `IdentityChanged` for complete identities that differ.
- Admitted custody records no longer discard `from_claim` errors. A construction refusal is reported as `CannotConstructSubject(ClaimAuthorityUnavailable(..))`; only the later exact-absence observation remains an `Assessed(Refused)` path.
- The legacy common-directory ownership mismatch is internally represented as `SourceCommonDirectoryBinding / OwnershipUnproven` and continues to project as legacy refusal.
- `RecordingProbe` counts source-authority observations independently of exact-absence observations. Regressions cover every existing source/common/worktree field mapping, unavailable source/common observations, unavailable repository authority, binding disagreement, and the report-colour distinction.

## Frozen genuine-red control

- Exact base tree: `84a48a4cff85fc7b1aba50c3a569ae79f6074a52`.
- Control: `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc3a2-genuine-red-control.patch`.
- SHA-256: `c5c235432c4e79ac00dbbe898e76d28ad9ed73208e9c2329b76ebb24fe40b01c`.
- The patch was structurally preflighted against that exact base through a temporary Git index. Its Cargo compile/run result is deliberately unclaimed and remains operator evidence.
- The control adds `inc3a_control_persisted_authority_failure_is_typed`. On the untouched base it should compile and execute, then fail because the persisted source-observation failure is still reported as `Assessed(Refused)` rather than the typed construction refusal.

```text
git apply docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc3a2-genuine-red-control.patch
CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked inc3a_control_ -- --nocapture
```

## Operator gates

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**
