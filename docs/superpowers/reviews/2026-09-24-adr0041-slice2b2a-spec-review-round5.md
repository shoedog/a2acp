# ADR-0041 Slice 2B2a spec review — extension round 5 (delta only)

**Date:** 2026-09-24
**Authority:** an owner-authorized cap extension, after the owner's caller-pinned digest decision (2026-09-24)
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp`, through the operator binary at
merge `d2cbf4e0`
**Execution:** `exec-c28aaf8730ca87dffbc075533064e4a5` / `attempt-6f9a99cffd623b5af74caddfd3f39668`
**Reviewed commit:** `c35a012031f08bb30dc870217679704bd8228d85` (tree `f8e9dcc73cd835083a64f90ee7998a25eee4c1b4`)
**Reviewed task SHA-256:** `1c378ce20a10dd2c30e0db885b2cae5856c0c09095f291e2a502de3c77e31164` (revision 5)
**Brief lint:** zero findings
**Raw result:** `/private/tmp/a2a-bridge-slice2b2a-spec-review5-20260924/result.md` (SHA-256
`6c503fd8e59e1e28e332108c9fe6adbd0f82337c2ee2518ff1e491502eafcbe6`). The node completed with a clean terminal and
complete cleanup.
**Verdict:** `REJECT`.

- Round-4 W1, S1, and S2: RESOLVED. W1 is resolved within the owner's boundary: a mismatching pin refuses before
  execution, and an exactly pinned wrapper is the caller's recorded decision.
- New: WRONG 4 / SMELL 1; BLOCKER 4 / DEFER 1.

## WRONG (all folded)

1. **2B2 had no route or digest input for mandatory admission.** **Fold:** 2B2 revision 8 takes a
   `GitRouteRequestV1` on its public entry point and adds control 31.
2. **The route-rule file was not bound to the descriptor whose bytes are hashed.** **Fold:** a single no-follow
   open with `fstat` facts, a post-audit identity binding, and hashing from the same descriptor; A5e.
3. **No control discriminated the pre-spawn and post-exit rehash.** **Fold:** A5f and A5g, in-place same-length,
   mtime-restored rewrites.
4. **`/usr/bin/git` was both forbidden and supported.** **Fold:** the runner has no default, fallback, `xcrun`, or
   `PATH` search, and executes exactly the caller-supplied admitted route.

## SMELL

1. The roadmap cursor was stale. **Fold:** updated.

## Classification

The four WRONG are closed, bounded consequences of the newly chosen mechanism, not repeats of earlier findings. They
are folded as 2B2a revision 6 and 2B2 revision 8, and go to one more delta round under the standing extension
authorization.
