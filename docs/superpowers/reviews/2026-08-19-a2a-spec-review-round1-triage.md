# A2a round 1 triage — 17 findings, and they cluster

5 BLOCKER, 10 MAJOR, 2 MINOR. Verdict: not ready to plan.

Raw count went UP versus A2 v2's 8. That is not automatically bad — A2a v1 is a
fresh artifact, so this is its round 1, and the fair comparison is A2 v1's
round 1, which returned 11. But the count is not the interesting part. The
cluster is.

## Two findings verified by direct experiment

### F1 (BLOCKER) — CONFIRMED, and it is a hard dead end as written

The spec authorizes `libc.workspace = true` in `bridge-worktree`'s
`[dev-dependencies]` while listing `Cargo.lock` as "do not modify," and mandates
`--locked` on every gate. Those cannot all hold.

Measured in the fold worktree at `c637e493`, with a before/after control:

| State | `cargo metadata --locked` | Result |
|---|---:|---|
| base (control) | exit 0 | passes |
| + `libc` in bridge-worktree dev-deps | **exit 101** | `error: cannot update the lock file ... because --locked was passed` |
| restored | exit 0 | passes |

The required lockfile delta is exactly **one line** — `"libc",` added to the
`bridge-worktree` dependencies list at `Cargo.lock:686`. No version resolution
changes; `libc 0.2` is already locked for `bridge-acp`, `bridge-core`,
`bridge-store`, and the binary. Total diff: 1 insertion, 0 deletions.

So the reviewer's suggested resolution is right and cheap: authorize the
one-line lockfile update, require no version-resolution change, and give it a
worksheet row. Fold as directed.

### F2 (BLOCKER) — CONFIRMED by reading the spec's own input contract

`A2A_SCAN_EXPECTED_MOUNT_ID` is listed as a required input, described as "the
exact mount identity captured by the read-only preflight" — but
`attested_scan_fixture_preflight` is the thing that *discovers* it. A first
preflight cannot supply it and must reject. Circular as written.

### F4 (BLOCKER) — CONFIRMED by reading the criteria

AC25 requires the matrix to pass on both attested APFS and ext4 fixtures. AC31
permits "exact totals **or explicit exclusions**." With one platform
unavailable, the same evidence both blocks and permits acceptance.

## The cluster that matters

| Cluster | Findings | Class |
|---|---|---|
| **Fixture attestation mechanism** | #2, #3, #7, #12, #13, and #4's platform rule | **OPEN-CLASS** |
| Seam/signature precision | #8, #9, #10, #11 | closed, enumerable |
| Custody and accounting | #1, #5, #15 | closed, enumerable |
| Design decisions | #6, #14 | closed, specific |
| Minor | #16, #17 | trivial |

**Six of seventeen findings are about one mechanism: the attested fixture-root
utility.** Each names a different instance of the same underlying gap — mount
discovery vs verification, same-mount object replacement, distro labelling,
JSON schema, injection boundary for synthetic coverage. That is the definition
of open-class: a new round would surface new instances rather than exhaust them.

**That mechanism is in A2a because I put it there.** When I split A2, I ruled
that the birthtime *capability* row went to A2b but the fixture attestation
*mechanism* stayed in A2a, reasoning that A2a drives real directory enumeration
and this lane already shipped a defect that passed on APFS and overlayfs and
failed only on ext4.

The reasoning was sound for A2b. It is weaker for A2a, and I did not check the
distinction at the time:

- A2a **adds no new filesystem observation**. It preserves `read_dir`,
  `read_sidecar`, `read_custody_record_in`, and pin-open semantics exactly; its
  own spec forbids behavior change and forbids genuine runtime red.
- The inode-reuse defect this lane paid for came from **new** custody behavior,
  not from a behavior-preserving refactor.
- `[MEASURED]` CI already runs `cargo llvm-cov --workspace` on `ubuntu-latest`,
  which is an existing uninstrumented ext4 control for the `bridge-worktree`
  suite. The macOS CI job covers `bridge-store` only, so APFS coverage for this
  crate is the host gate — which is where A2a's evidence lands anyway.

So A2a is buying a bespoke, six-findings-deep attestation protocol to prove a
filesystem-variance property that (a) it does not change and (b) CI already
exercises on ext4 for free.

Note the second-order cost: F6 (the ordering-oracle finding — two independent
`read_dir` traversals have unspecified relative order, so a real-filesystem
equality oracle can fail spuriously) exists **only because** conformance is
proven against real filesystems. With injected deterministic name streams, F6
does not arise.

## Recommendation

Fold the closed clusters into A2a and lift the attestation mechanism out.

A2a proves its actual claim — one engine, two projections, equivalent
output — with **injected deterministic sources**, which is stronger evidence for
that claim than a real filesystem provides, because it can force pin failure,
malformed sidecars, and exact orderings on demand. Real attested-filesystem
conformance becomes its own slice, sequenced with A2b's platform matrix where
the capability question actually lives.

This is not a third fold of a converging artifact; it is a scope correction of
a mis-assigned concern, and it is the same correction the reviewer's own #15
gestures at ("split attestation or descriptor work into a narrower slice").
