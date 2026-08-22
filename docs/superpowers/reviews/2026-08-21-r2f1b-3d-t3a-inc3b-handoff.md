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

- [x] `cargo fmt --all -- --check` — **exit 0**
- [x] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **exit 0**
- [x] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **exit 0**
- [x] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **exit 0**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **exit 0**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **exit 0**


---

## Operator evidence — filled 2026-08-22

**Implementation commits:** `db3066ff` (3B) then `64681e0e` (repair)
**Base:** `f7e2e8e2`

All gates green: fmt 0; clippy `--workspace --all-targets --locked -D warnings` 0;
full workspace suite 0 with zero failures; `bridge-worktree` **331 passed** against
the base control's 323 — +8, zero failures either side; hygiene 0.

**Counted 800 nonblank added Rust lines against an 850 cap.**

### The increment-closing criterion is met

`ClaimAuthorityObjectV1::Root` has **6** production construction sites and
`ClaimAuthorityUnavailableReasonV1::OwnershipUnproven` has **3**, both counted
outside `mod tests`. **No object or reason arm remains dormant.** The vocabulary A1
landed in PR #60 is now fully exercised by production, which is what increment 3
existed to finish.

### The frozen control is behavioural red, verified independently

SHA-256 `6c8dde1e497b5e45bcec0b443b88b630ea528e294aeeaa8273b131affe445a5b`
recomputed from the patch: identical. Applied to a detached worktree at the
untouched base and run:

```
test sweep::tests::inc3b_control_degraded_claim_root_never_reaches_exact_absence ... FAILED
test result: FAILED. 0 passed; 1 failed
```

**Zero compile errors** — it compiles and runs, then fails asserting that a degraded
root reports `ClaimAuthorityUnavailable(Root, …)` instead of reaching the probe.
That is precisely the rescope design's first named behavioural-red item for this
increment.

### Sizing — the derived cap held, the inherited one would not have

Sol projected 575 against the design's inherited 600 anchor. The operator replaced
that cap with 850, derived by applying this lane's worst observed
projection-to-delivered ratio (1.48x) to the measured projection. Actual: **800**,
a **1.39x** miss — inside the observed 1.21–1.48x band.

The inherited 600 would have fired before a line was written and forced a split of
work that fits comfortably. This is the first cap in the lane set from the measured
ratio rather than guessed, and it held.

### Two operator errors on this slice, recorded

- The first REJECT named "a single mis-scoped path". I judged that a one-token
  operator repair rather than a dispatch. **The premise was false**: it was one
  blocker *visible while the crate did not compile*. Applying it locally revealed
  two more first-execution failures. A non-compiling crate does not mean one
  defect; it means the count is unknown.
- Both fixture failures were then root-caused before dispatch rather than handed
  over as symptoms: `validate` requires exact structural equality between the
  record's envelope worktree and the claim's worktree, and the fixtures degraded
  only the claim copy.

### Limits

- Attests the tree at `64681e0e` only.
- `EXACT_ABSENCE_POLICY_READY_V1` remains `false`; readiness is still the sole
  remaining production gate, unchanged since slice B narrowed it.
- T3b still owns acting on this evidence: the report carries ordered historical
  evidence, not authority.
