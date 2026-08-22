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

- [x] `cargo fmt --all -- --check` — **exit 0**
- [x] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **exit 0**
- [x] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **exit 0**
- [x] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **exit 0**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **exit 0**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **exit 0**


---

## Operator evidence — filled 2026-08-21

**Implementation commit:** `01ef7a91` · **Base:** `b73e0a5a`

All gates green: fmt 0; clippy `--workspace --all-targets --locked -D warnings` 0 with
zero warnings; full workspace suite 0 with zero failures; `bridge-worktree`
**323 passed** against the base control's 320 — +3, zero failures either side; hygiene 0.

**Counted 353 nonblank added Rust lines against a 380 cap**, measured independently
per file.

### The frozen control is behavioural red, verified independently

Recorded SHA-256 `c5c235432c4e79ac00dbbe898e76d28ad9ed73208e9c2329b76ebb24fe40b01c`
recomputed from the patch: identical. Applied to a detached worktree at the untouched
base `b73e0a5a` and run:

```
test sweep::tests::inc3a_control_persisted_authority_failure_is_typed ... FAILED
test result: FAILED. 0 passed; 1 failed
```

**Zero compile errors** — the test compiles and runs, then fails its assertion that a
persisted authority failure is typed `CannotConstructSubject(ClaimAuthorityUnavailable(..))`.
That is genuine behavioural red, which is the correct evidence for this half; 3A-1's
port was behaviour-preserving and correctly had none.

*Operator note:* the first control run used the filter `inc3a2_control_` and matched
nothing — 321 filtered out, exit 0. That result was **inadmissible**, not a pass: the
prefix is `inc3a_control_`. Re-run with the correct filter, as recorded above.

### Limits

- Attests the tree at `01ef7a91` only.
- `EXACT_ABSENCE_POLICY_READY_V1` remains `false`; readiness is still the sole
  remaining production gate.
- Two non-blocking SMELLs were raised and neither has a demonstrated incorrect output:
  one dead branch, one cosmetic error-string collapse. Carried, not repaired.
- 3B still owns the retained root identity, Host Git brackets, the sixteen-row degraded
  matrix, and persisted-record integration evidence.
