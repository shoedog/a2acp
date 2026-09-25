# ADR-0041 Slice 2B2a spec review — delta round 7

**Date:** 2026-09-24
**Authority:** an owner-authorized cap extension (2026-09-24)
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp`, through the operator binary at
merge `d2cbf4e0`
**Execution:** `exec-e6acee3e6498197f4f8687e841c8ab4b` / `attempt-5dc9f4e363bfb542069c1efbe35e6115`
**Reviewed commit:** `cc6f3b174fb018a11d3381df5730f00e78ff76f0` (tree `730e828a5bafaf18dd471dc62de398828f3fd75f`)
**Reviewed task SHA-256:** `9dc61eb066f8ad78c0c8ef340993816c5a7697425b85d63ef4b239c9a8db3056` (2B2a revision 7)
**Brief lint:** zero findings
**Raw result:** `/private/tmp/a2a-bridge-slice2b2a-spec-review7-20260924/result.md` (SHA-256
`79b6f27e0511845db5a46b7166f33d5f81d78d933484381f369e6c681ed38131`). The node completed with a clean terminal and
complete cleanup.
**Verdict:** `REJECT`.

- Round-6 items: W1, S2, and S3 RESOLVED; S1 UNRESOLVED.
- New: WRONG 2 / SMELL 2; BLOCKER 2 / DEFER 2.

The reviewer confirmed that the full route-fact set is observable on macOS and Linux, and that observing it does not
advance ctime, so unchanged files are not falsely refused.

## WRONG (folded in revision 8)

1. **A5e could not discriminate the binding, the per-field comparison, or the pre-spawn route-rule re-run.** The
   mode/ctime change was caught by more than one guard. **Fold:** A5e, A5e-t, and A5e-r, one guard each.
2. **ctime masked the A5f and A5g digest rehash controls.** **Fold:** a test-only fact-observation seam holds the
   facts constant.

## SMELL (folded)

1. Stale status line. **Fold:** fixed.
2. A5g readiness came before `exec`. **Fold:** readiness is signalled after process-image replacement.

## Classification

The production requirements have converged; this round found no production-behavior defect. The recurring class, in
rounds 1, 2, 5, 6, and 7, is control discrimination, which is open-ended at spec level because each new guard can
mask other controls. Following the convergence rule, it is escalated to the owner rather than extended to round 8.
