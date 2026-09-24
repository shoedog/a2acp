# ADR-0041 Slice 2B2 spec review — round 1

**Date:** 2026-09-24
**Round cap:** 1 of 2 admitted rounds consumed
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp`. The operator binary
`a2a-bridge-operator-main/target/release/a2a-bridge` was used at merge `d2cbf4e0`, and `models` confirmed the model,
effort, and mode before billing.
**Reviewed commit:** `1c22d0d0f296e07a0aa82bc8c2476270b5c310f2` (tree `9dd9a41db50452bccee42fa9e77f182ebae99be6`)
**Reviewed task SHA-256:** `4eabc46d439d494dc5048fcefb617eb86805dc394130c109f9e625fe284bf6b0` (revision 2)
**Brief lint:** zero findings
**Raw result:** `/private/tmp/a2a-bridge-slice2b2-spec-review1-20260924/result.md` (SHA-256
`f10f569a76c81f3b8d1fd2b9ec726ec219d3381187ebfb63e8a89d2e3dd3f6b2`). The node completed with a clean terminal and
complete cleanup.
**Verdict:** `REJECT`. WRONG 5 / SMELL 9; BLOCKER 5 / DEFER 9. The population is closed and enumerable.

The reviewer confirmed that the revision-2 folds (all-object `rev-list`, `--no-dangling`, single pack production,
in-crate real-Git tests, the restored test path, and the broader ledger) are directionally correct, and that the
owner-approved 2B2b split keeps 2B2 coherent. It audited probes P1/P2 as supplied evidence without rerunning them.

## WRONG findings and disposition

Each WRONG was checked against the source before folding.

1. **W1: the filesystem seams cannot create a descriptor-relative directory tree.** Verified: `fs_custody` has no
   `mkdirat`, and `PinnedDirectoryV1`'s `file` field is private. **Folded:** task §6 owns two `PinnedDirectoryV1`
   methods, `create_new_child_directory` and `open_existing_child_directory`. §8 ownership is narrowed to exactly
   those methods, and control 23 covers ancestor replacement.
2. **W2: the receipt cross-check permitted artifact-name substitution.** Verified: `CustodyEnvelopeSealReceiptV1`
   carries `artifact_name` and `manifest_digest`, and binding compares name sets only. **Folded:** §6 builds the
   expected receipt from the exact context and the destination's ciphertext receipt and requires whole-value
   equality. Control 15 covers the swap.
3. **W3: the bytes fed to `index-pack` were not bound to the recorded pack identity.** **Folded:** §5 step 1 streams
   through a hashing and counting writer from the retained descriptor and requires equality. Control 17 covers it.
4. **W4: compile-fail doctests were placed in a `tests/` target.** Verified: Cargo runs doctests only for library
   targets. **Folded:** the doctests live on `src/custody_export.rs` items, and §9 adds a `--doc custody_export`
   gate. Control 24 covers it.
5. **W5: controls could not meet the single-guard rule.** **Folded:** the lazy-fetch control is split into 8a–8d with
   a test-only `file://` promisor fixture and explicit bypasses. The no-mutation check becomes control 21, which
   targets a new §2 scratch/source disjointness preflight, and the byte comparison is kept as a separate observation.

## SMELL findings and disposition

All nine are folded because each is cheap:

1. unverified seal rename → `SealPublicationUnverified`, control 14;
2. manifest accessors → crate-private `custody_seal.rs` accessors, added to ownership;
3. ledger precision → logical-byte ledger, enumerated per-Git-child upper bounds, post-exit re-measure;
4. object-store `info/` reads → deliberate, documented exception covered by recursive pinning;
5. plaintext receipt source → exporter-owned `CustodyEnvelopeSourceValidatorV1` wrapper, control 16;
6. role replay → control 19;
7. ext4 lane → `statfs` magic `0xEF53` admission;
8. Git binary → identity and SHA-256 recheck before each spawn, control 22;
9. canonical-manifest ceiling → bounded streaming preflight before `derive`, control 11.

## Convergence

The pre-review audit on revision 1 found 7 WRONG. Round 1 on revision 2 found 5 new WRONG, none repeating an earlier
finding. The findings are closed and each names its bounded fix, so they are repaired on the same artifact as
revision 3. Round 2 is the final admitted round. If round 2 rejects, classify before acting: fold a converging closed
residue with a one-line extension disclosure, or park an open-class population for design.

The review did not authorize implementation, push, merge, cleanup, 2B3 restore, remote/provider effects, or
running-operator mutation.
