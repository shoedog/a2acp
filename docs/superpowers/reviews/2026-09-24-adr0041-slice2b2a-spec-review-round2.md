# ADR-0041 Slice 2B2a spec review — round 2 (final admitted round)

**Date:** 2026-09-24
**Round cap:** 2 of 2 consumed; the owner authorized an extension if needed (2026-09-24)
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp`, through the operator binary at
merge `d2cbf4e0`
**Execution:** `exec-06d86373e847345cb300f82bc16cc962` / `attempt-21e0eb95d7a54819c6752cb242a9d3bf`
**Reviewed commit:** `be10c255f9bd51b52db627d95268fd1614eb59ff` (tree `b73443781537097824132cdaed171bf887239dfa`)
**Reviewed task SHA-256:** `dae999256a950bc825b2b4a98feca635da5cc1a1577973928b6bc33c126c5e10` (revision 2)
**Brief lint:** zero findings
**Raw result:** `/private/tmp/a2a-bridge-slice2b2a-spec-review2-20260924/result.md` (SHA-256
`9ae16ebf27e61b9b9d4f9eb34951d2872ef7aa81fede06220c975ca97205fdbb`). The node completed with a clean terminal and
complete cleanup.
**Verdict:** `REJECT`.

- Inherited: 10 RESOLVED / 0 UNRESOLVED / 0 DEFERRED.
- New: WRONG 2 / SMELL 5; BLOCKER 2 / DEFER 5.

## WRONG findings (folded in revision 3)

1. **Canonicalization erased a final symlink before admission.** **Fold:** a no-follow `lstat` of the final component
   runs before canonicalization. The A5 row carries a canonicalize-first mutation.
2. **A10 permitted false greens for full-duplex I/O.** **Fold:** a 1 MiB stdout, 1 MiB stderr, 1 MiB stdin fixture,
   with three serialization mutations that must each time out.

## SMELL findings (folded)

1. **A11d `GIT_DIR` observability.** **Fold:** environment-map assertion plus a planted template.
2. **A14 scope against the 35 base `unsafe` sites.** **Fold:** new-site inventory plus a frozen base count.
3. **Route evidence role/order schema.** **Fold:** domain-tagged primary digest plus an ordered alternate array; A18.
4. **`xcrun` no-execution unproven.** **Fold:** implementation-time probe P7 as a stop condition.
5. **Ubuntu fixed point unmeasured.** **Fold:** host probe P5 on Debian found distinct byte-identical files; the
   fixed-point rule now admits byte-identical copies that pass the route rule; lanes are recorded before code.

## Additional host-probed defects folded (not reviewer findings)

- **P6:** the implement/verify container runs as root, and temp-directory fixture ancestors can never pass a
  production audit, so the specified tests could not admit any Git binary. **Fold:** anchored fixture and root-aware
  system test profiles; A17.
- **P6b:** distro Git 2.39.5 rejects `--no-lazy-fetch`. **Fold:** minimum-version lanes and a stop condition.

## Convergence

WRONG went from 5 (round 1) to 2 (round 2), none repeating, all closed. This is a converging population. Under the
owner's extension authorization, revision 3 is reviewed in one disclosed extension round.
