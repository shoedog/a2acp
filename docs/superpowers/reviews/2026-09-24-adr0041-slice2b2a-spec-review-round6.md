# ADR-0041 Slice 2B2a spec review — delta round 6

**Date:** 2026-09-24
**Authority:** an owner-authorized cap extension (2026-09-24)
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp`, through the operator binary at
merge `d2cbf4e0`
**Execution:** `exec-f9bdf7d519666ae4e19743c678d0fd2e` / `attempt-9d85e2fa28ed08e557af3c6cbe8d2d33`
**Reviewed commit:** `fb7c4aab26e1d548a9d509821ece7fdad105c2da` (tree `aa48b1cd46181335fa58fceff4a6555a5f7b61d3`)
**Reviewed task SHA-256:** `6c9224f83a9133a825d0c4fbddb62c4bbc8b93e2a8c14a54f706128810f4be1a` (2B2a revision 6)
**Brief lint:** zero findings
**Raw result:** `/private/tmp/a2a-bridge-slice2b2a-spec-review6-20260924/result.md` (SHA-256
`50033a6d259ebfa3f7da420e4d2b2f14ed2440612dc55db1685684486505599a`). The node completed with a clean terminal and
complete cleanup.
**Verdict:** `REJECT`.

- Round-5 items: W1, W3, and W4 RESOLVED; W2 and S1 UNRESOLVED.
- New: WRONG 1 / SMELL 3; BLOCKER 1 / DEFER 3.

## WRONG (folded in revision 7)

1. **The R5 W2 residue: the binding omitted mutable route-rule facts.** A same-inode `chmod 0777` or ACL change
   between the audit and the binding preserves device, inode, size, and mtime.
   **Fold:** the binding compares the full route-fact set (device, inode, type, uid, gid, mode, size, mtime, ctime);
   the route rule is re-run before every spawn; A5e gains same-inode and pre-spawn arms.

## SMELL (folded)

1. Stale status tokens. **Fold:** all updated.
2. A5g fixture shape could hit `ETXTBSY`. **Fold:** a shell-wrapper `exec /bin/sleep` protocol with a
   rewrite-succeeded assertion.
3. Route-request visibility across a `pub` 2B2 entry point. **Fold:** deferred to the 2B2 review and recorded in
   2B2 §2.

## Classification

WRONG went from 4 (round 5) to 1 (round 6). The remaining finding is a refinement of the same item with a
one-paragraph fix. This is converging. It goes to a narrow delta round 7.
