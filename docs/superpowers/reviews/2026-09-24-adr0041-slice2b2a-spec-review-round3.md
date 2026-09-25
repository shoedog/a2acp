# ADR-0041 Slice 2B2a spec review — extension round 3

**Date:** 2026-09-24
**Authority:** an owner-authorized cap extension (2026-09-24), beyond the two admitted rounds
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp`, through the operator binary at
merge `d2cbf4e0`
**Execution:** `exec-a50df3f1a1a3db77e16c609c8585b2e7` / `attempt-a55cbc41a816db338d01e73eb1e59aae`
**Reviewed commit:** `81b6a51f1ba92840be8a680e255ec4539be66110` (tree `46b50fbd127b9be4e228b7740f27cb18a9721a25`)
**Reviewed task SHA-256:** `9cda693eb1523d5ba8db59c3e5fc97afb54b5db13c2cc690dd80e1f8eeaf57a4` (revision 3)
**Brief lint:** zero findings
**Raw result:** `/private/tmp/a2a-bridge-slice2b2a-spec-review3-20260924/result.md` (SHA-256
`8d3ec189a88a841af385ea53852a4a6a878a7787aaaf9d22cf617727ac8d9731`). The node completed with a clean terminal and
complete cleanup.
**Verdict:** `REJECT`.

- Round-2 items: 5 RESOLVED / 0 UNRESOLVED / 2 DEFERRED (S4 and S5, both fail-closed pre-code probes).
- New: WRONG 1 / SMELL 3; BLOCKER 1 / DEFER 3.

The reviewer confirmed these revision-3 changes:

- the pre-canonicalization `lstat` and the A10 full-duplex fixture (it notes the repository's own recorded ~64 KiB
  pipe deadlock);
- the A11d mutations;
- the A14 two-part inventory, against the exact 35-site base count, with `syn` `full`/`visit` present;
- the A18 evidence schema;
- that both test admission profiles are compile-time test-only;
- the `/opt/git` 2.54 lane, from the Containerfile.

## WRONG (folded in revision 4)

1. **Byte-identical fixed-point admission can admit a copied wrapper that dispatches to an unadmitted executable.**
   This contradicted §1's categorical "cannot execute an unadmitted binary". The reviewer offered two bounded repairs:
   a digest allowlist, or narrowing §1.
   **Fold:** §1 is narrowed. The runner's own `exec`s target only admitted routes, and an admitted route's descendants
   are trusted as a privileged installation (HL3, extended). The fixed point is documented as a heuristic.
   **Rationale:** a digest allowlist breaks on every OS or Git update. An identity-only fixed point already admitted
   the same wrapper class. The owner's 2026-09-24 ruling puts privileged actors out of scope. The owner may still
   choose the allowlist instead.

## SMELL

1. P7 (`xcrun` executes nothing) is unexecuted. It remains a fail-closed pre-code probe.
2. The GitHub ubuntu runner's Git is unmeasured. It remains a fail-closed pre-code inventory.
3. §11 was stale. **Fold:** rewritten.

## Convergence

WRONG counts were 5 → 2 → 1, none repeating. The loop is converging. The revision-4 delta goes to a narrow extension
round 4 under the same owner authorization.
