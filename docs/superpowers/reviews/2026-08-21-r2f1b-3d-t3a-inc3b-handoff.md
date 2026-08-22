# T3a increment 3B handoff — retained root and bracketed Host Git

## Candidate checkpoint

- Implementation base: `f7e2e8e289b432a708bf954ca393a29958d38c84`.
- This implementation candidate changes only `crates/bridge-worktree/src/sweep.rs`, `crates/bridge-worktree/src/sweep/checked_scan.rs`, `crates/bridge-worktree/src/host_git.rs`, this handoff, and its frozen control. Manifests and `Cargo.lock` are untouched.
- The current candidate adds 800 nonblank physical Rust lines, measured from the final formatted diff; this is within the 850-line cap.
- No Cargo build, test, clippy, or repository-hygiene gate was run in this implementation container. All listed operator gates remain pending.
- The initial compile error in `checked_scan.rs` prevented every slice test from executing. The two degraded-worktree fixture failures exposed after that repair are first-execution results, not regressions: `WorktreeCustodyRecordV1::validate` requires exact structural equality between envelope `worktree` and claim `worktree`, so degrading only the claim made `encode_canonical` reject the inconsistent record with `ClaimIdentityMismatch`. Both fixtures now degrade the two copies identically.

## Implementation

- A private retained-root carrier derives `Stable(DirectoryIdentityV1)` only when the retained directory enumeration, the pinned custody directory, and the final named root have complete agreeing identities. It preserves the existing public root-observation classification.
- Custody claim construction validates the root’s outer and embedded canonical paths, absoluteness, complete identity, and match to that retained authority. Custody candidates retain the matched root; legacy candidates retain `None` and keep their existing source/common-directory bracket.
- Root construction failures are typed as `Root` with the existing reason vocabulary. Together with 3A’s source/common-directory/binding work, every report object and reason arm now has a real production constructor; no vocabulary arm remains dormant.
- Host Git owns both complete brackets: filesystem identities for source, optional custody root, and common directory; repository authority tied to the common directory; no-follow target observation; `git worktree list --porcelain -z`; then the same filesystem and authority checks followed by the target observation. Any later bracket failure remains `Assessed(Refused)`.
- The persisted-record evidence includes the literal sixteen-row degraded matrix, distinct source/root/common-directory replacements, two-repository binding substitution, target/registration/both-absent Host Git controls, degraded and historical-complete worktrees, and root replacement during the Git bracket. It also covers a complete but disagreeing retained-root observation as `Root / IdentityChanged`. Each new persisted-record regression snapshots and compares canonical custody bytes.

## Bounded effect audit

The new path remains `checked scan → report projection → candidate construction → probe observation`. Its added edges only perform record decoding, canonicalization, directory identity observation, `git rev-parse`, `git worktree list --porcelain -z`, allocation, collection, and tracing. It adds no removal, pruning, unlink, rename, custody publication, transition, settlement, or backend-cleanup edge.

## Frozen genuine-red control

- Exact base tree: `f7e2e8e289b432a708bf954ca393a29958d38c84`.
- Control: `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc3b-genuine-red-control.patch`.
- SHA-256: `6c8dde1e497b5e45bcec0b443b88b630ea528e294aeeaa8273b131affe445a5b`.
- The patch was structurally preflighted against that exact base through a temporary Git index. Its Cargo compile/run result is deliberately unclaimed and remains operator evidence.
- The control adds `inc3b_control_degraded_claim_root_never_reaches_exact_absence`. On the untouched 3A base it should compile and execute, then fail because an incomplete claim root is ignored and reaches exact absence instead of producing `CannotConstructSubject(ClaimAuthorityUnavailable(Root, IdentityIncomplete))`.

```text
git apply docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc3b-genuine-red-control.patch
CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked inc3b_control_ -- --nocapture
```

## Operator gates

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**
