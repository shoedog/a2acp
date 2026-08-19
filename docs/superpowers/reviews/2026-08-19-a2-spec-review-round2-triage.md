# Round 2 triage — one blocker refuted, eight valid

## REFUTED — finding 1 (BLOCKER): "rustfmt will rewrite the normative block"

The reviewer claims the v2 seam block manually splits the `read_legacy` return
type and that rustfmt will rewrite it, so byte-for-byte and the fmt gate cannot
both hold.

**Measured, with a same-environment control.** Toolchain pinned by
`rust-toolchain.toml` to 1.94.0; `rustup run 1.94.0 rustfmt --version` and bare
`rustfmt --version` are the same binary, `rustfmt 1.8.0-stable (4a4ef493e3
2026-03-02)`. No `rustfmt.toml` in the repo, so defaults apply.

Running the gate's own mode on the two blocks:

| Block | `rustfmt --check --edition 2021` | Meaning |
|---|---:|---|
| **v2** (current spec) | **exit 0** | gate PASSES; block is already normalized |
| v1 (pre-fold) | exit 1 | gate FAILS; the original finding was real |

The v1→v2 control shows the probe discriminates: it catches a non-normalized
block and passes the normalized one. `rustfmt` is also a no-op on the v2 block
(byte-identical output), so it is a fixed point, not a coincidence.

The `read_legacy` split the reviewer objects to is **rustfmt's own output**. The
single-line form measures exactly 100 characters, at `max_width`, and the
formatter chose to wrap it. Round 1's finding was correct; round 2 re-raised it
against the already-fixed block and is wrong.

No action. Do not "re-normalize" this block — doing so would reintroduce the
round-1 defect.

## VALID — the remaining eight

| # | Sev | Finding | Probe |
|---:|---|---|---|
| 2 | BLOCKER | Mandatory APFS/ext4 matrix defines no fixture-root supply or filesystem attestation; a default tempdir could be tmpfs/overlayfs and be reported as ext4 | CONFIRMED — v2 contains no fixture-root, mount, or fs-type mechanism; only one prose mention of "mount" at line 193 |
| 3 | MAJOR | Deterministic pin failure is injectable only via `scan_worktree_records_with_pin_opener` (action projection); the exact-report route hardcodes `FilesystemCompatibilityPinOpenerV1`, so matrix conformance can be claimed while testing one projection | CONFIRMED — v2:249/251/414 give the seam to the action path only |
| 4 | MAJOR | The four runtime-red tests have no reproducible tree to run against base production code; the two-commit protocol defines only candidate + handoff-only evidence commit | Design-level, follows from the R3 fold |
| 5 | MAJOR (SMELL) | `std::fs::ReadDir` gives no inspectable identity for the enumerated directory, so slice B cannot populate `retained_enumeration_object` without replacing the mechanism | Design advice, non-gating per the reviewer's own resolution |
| 6 | MINOR (SMELL) | Source-incompatible public break lands at `0.3.1` with only handoff prose preventing patch publication | Re-litigates V4, which was settled deliberately. Not a new finding |
| 7 | MINOR (SMELL) | Scoped tracing capture is mandatory but `bridge-worktree` has no `tracing-subscriber` dev-dependency and the manifest is read-only | CONFIRMED — dev-deps are `bridge-coordinator`, `bridge-controller` only |
| 8 | MINOR (SMELL) | The capability test passes for `Some` or `None`; captured `cargo test` output does not reveal which | Follows from the V3 fold |
| 9 | MINOR (WRONG) | Mutation inventory presents both final `deepest_existing_path` bracket calls as unconditional; the comparator can return `CannotProve` first | `deepest_existing_path` exists at `bridge-core/src/fs_custody.rs:1511`, used as a resolver at :1777 |

## Convergence classification

| | Round 1 | Round 2 |
|---|---:|---:|
| BLOCKER | 3 | 1 valid (1 refuted) |
| MAJOR | 6 | 3 |
| MINOR | 2 | 4 |
| **Total** | **11** | **8 valid** |

Fewer, smaller, and non-repeating — **converging** by the steering definition.
The only repeat is refuted; #6 is a re-litigation of a settled choice.

**But the shape matters more than the count.** Findings 2, 4, 8 and part of 3
are not about the slice's behavior — they are about its *evidence apparatus*,
and each one exists because a round-1 fold added machinery. R7 produced the
platform matrix, which produced #2 and #8. R3 produced the two-commit protocol,
which produced #4. R4 produced the scan engine, which produced #3.

That is the signature of a slice whose proof obligation spans more than one
commit can carry: two production projections x two filesystems x genuine-red
evidence x mutation audit, all in one artifact with a 1,650-line cap.
