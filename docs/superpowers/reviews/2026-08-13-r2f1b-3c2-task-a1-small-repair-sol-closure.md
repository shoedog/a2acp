# R2f1b 3c2 Task A1 small-repair Sol/xhigh closure

Date: 2026-08-13

Reviewed commit: `6616753bf479d8775381eb9ef1d7237f5660514c`

Exact parent: `bc262ad466b45470cd44fceda8224a36b2ba77b2`

Execution: `exec-0ee009bac4882d0bcc4da06badd4777f`

Attempt: `attempt-b1a26506540f87cea5dc44410034fa45`

Terminal artifact: 8,869 bytes, SHA-256
`b17ee8ce7e76e40643c806a33b499f42edd709d5343b22970eac40358e012df0`.

The review was one hard-read-only Codex `gpt-5.6-sol` / xhigh pass. It did not
edit, build, test, invoke a provider beyond the review itself, or access the
network. The configured LSP service was unavailable, so the reviewer used
bounded exact-symbol source search for caller containment.

## WRONG findings

1. **WRONG - BLOCKER: restoration can move an unrelated custody object into
   the target.** After observing custody identity, restoration is still a
   name-based no-replace rename. A namespace peer can move captured B away and
   create foreign C at the deterministic custody name before the restoring
   rename. The function then moves C into the authoritative target and returns
   `Unknown`; the protective result does not undo the incorrect mutation.

   The bounded safe repair is to refuse restoration and retain debt unless an
   identity-linearized operation or an enforced operation lock excludes the
   peer. A deterministic red should substitute custody at `boundary(true)` and
   prove no second rename moves C.

2. **WRONG - BLOCKER: a failed capture can still be falsely classified
   `RefusedNoEffect`.** After an error-after-effect rename moves target to
   custody, a peer can hard-link the same object back to target. Target identity
   then equals the pre-rename identity, so the function reports
   `RefusedNoEffect` although custody remains occupied and the rename occurred.

   Without stronger linearization proof, every post-attempt I/O error must be
   `Unknown`; target identity equality alone cannot prove no effect. A red
   should inject move-to-custody, hard-link-back, and `EIO` and require
   `Unknown`.

Both triggers are rare and dormant because the A1 surface has no production
caller, but both are constructible once armed. They are new instances of the
same uncooperative-namespace-peer class that previously parked Task A.

## SMELL findings

1. **SMELL - DEFER:** runtime-unsupported detail loses the useful reason by
   extracting and reformatting only the inner label. The typed outcome remains
   correct; preserve `error.to_string()` when this surface is next touched.
2. **SMELL - DEFER:** behavioral fail-first and actual non-Unix execution
   evidence remain incomplete. The new seam did not exist on the rejected
   parent, and no real Windows test ran.

## Inherited adjudication

- **Incomplete identity can mutate: FIXED.** Every unavailable, non-regular, or
  missing-birthtime path returns before the boundary and rename.
- **Failed capture can adopt foreign custody: FIXED for that exact mechanism.**
  Capture `Io` returns without inspecting custody or entering restoration.
- **Non-Unix API is absent: FIXED in source, Windows execution excluded.** The
  pure constructors are portable and the non-Unix capture stub returns
  `CompileUnsupported` without inspecting or mutating its arguments.

Scope is contained to `crates/bridge-core/src/fs_custody.rs`: 207 insertions,
28 deletions, 235 touched lines. Exhaustive source search found only definitions
and colocated tests for the new A1 surface; no persistence, wrapper, served
projection, production caller, or V3 activation was introduced.

Confidence: **92/100**. Deterministic execution of the two proposed race reds
and a real Windows gate would raise it. A checked-in threat model proving that
namespace peers cannot write during the operation would lower it. An enforced,
unbypassable identity-bound mutation primitive covering both rename windows
would collapse the blockers.

VERDICT: REJECT

SUMMARY: All three inherited WRONGs are fixed, but two unlinearized namespace races can still move or falsely classify custody objects; Windows remains an explicit verification exclusion.
