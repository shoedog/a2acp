# ADR-0041 Slice 2B2 spec review — round 2 (final admitted round)

**Date:** 2026-09-24
**Round cap:** 2 of 2 admitted rounds consumed; the cap is exhausted
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp`, through the operator binary at
merge `d2cbf4e0`
**Execution:** `exec-98b4dd6d404830cd45dff6f4a3d9140b` / `attempt-1b39724c67b7345c4c9a0c4f7c3ead48`
**Reviewed commit:** `a1177a66` (tree `ccc7be75c8fe109452d024ef7b7ccda5001dae16`)
**Reviewed task SHA-256:** `2da9912b2dd16908637c1d2ca8366aea6ad5b107b8098631ed6eafd19afba5d3` (revision 3)
**Brief lint:** zero findings
**Raw result:** `/private/tmp/a2a-bridge-slice2b2-spec-review2-20260924/result.md` (SHA-256
`ffee7ef52b7ca3706acfced8814a31ddd7dd0e8fe184c418c95e023e77f6bbc3`). The node completed with a clean terminal and
complete cleanup.
**Verdict:** `REJECT`.

- Inherited: 11 RESOLVED / 3 UNRESOLVED (S1, S7, S8) / 0 DEFERRED.
- New: WRONG 6 / SMELL 3; BLOCKER 6 / DEFER 3.

The reviewer confirmed that the pinned identities matched, read the complete four-file diff, and audited statically
without building. It confirmed the conservative index bound (the exact v2 maximum is `1032 + N·(H+16) + 2H`) and the
reverse-index formula.

## WRONG findings

1. **W1: no descriptor-relative create-new regular child.** `PinnedDirectoryV1` only opens existing regular
   children, and `publish_new_regular_child` needs a source already present in the directory. The staging files and
   `work/objects.pack` therefore had no owned creation route. **Closed; folded** (third §6 method).
2. **W2: seal-rename outcome contradiction.** "At or before the seal rename → no seal" overlapped
   `SealPublicationUnverified` for `UnlinkSourceOnly`, so S1 was reopened. **Closed; folded** (§6 arm narrowed;
   control 12 excludes unverified renames).
3. **W3: Git-child pathname resolution.** Children resolve `GIT_DIR`, `HOME`, and init targets by path after the
   parent's recheck, so a swap in between can redirect their writes. **Open-class; parked.**
4. **W4: `mkdirat` → `openat` substitution.** The opened directory is not bound to the one that was created.
   **Open-class; parked.**
5. **W5: Git binary check-to-exec.** `exec` reopens the path after the hash recheck, so S8 was reopened. **Open-class;
   parked.**
6. **W6: `0xEF53` is the magic for the whole ext family.** It cannot prove ext4, so S7 was reopened. **Closed;
   folded** (magic plus `mountinfo` fstype).

## SMELL findings

1. The lazy-fetch guard-off arm contaminates a reused source store. **Folded:** each arm uses a fresh store from an
   immutable template.
2. Control 11 did not discriminate preflight-before-derive ordering. **Folded:** a call-counter seam.
3. §10 still pointed at revision 2. **Folded.**

## Convergence classification at the cap

The WRONG counts ran 7 (audit) → 5 (round 1) → 6 (round 2). They are not decreasing. W3, W4, and W5 are new instances
of one class: check-to-use windows at path-addressed boundaries raced by an actor running as the same user. Round 1's
W1 and S8 already touched that class. The per-steering disposition is to fold the closed residue (W1, W2, W6, and all
three SMELLs) on the same artifact as revision 4, and to **park the open class for an owner threat-model ruling**
(task §14). No third round, restart, or silent extension was taken.

This review did not authorize implementation, push, merge, cleanup, 2B3 restore, remote/provider effects, or
running-operator mutation.
