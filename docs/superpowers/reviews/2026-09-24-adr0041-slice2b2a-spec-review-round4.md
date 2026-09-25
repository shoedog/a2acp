# ADR-0041 Slice 2B2a spec review — extension round 4 (delta only)

**Date:** 2026-09-24
**Authority:** an owner-authorized cap extension (2026-09-24)
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp`, through the operator binary at
merge `d2cbf4e0`
**Execution:** `exec-a43fac9a486ce3e6e42ca1e0d9662bc5` / `attempt-a1305192ba1225789efa0a88f51b84c7`
**Reviewed commit:** `c4175f470ef2f708b92fd985c880435f64b51d83` (tree `8d7898d341fee9f155a371369438faa2b0f60af5`)
**Reviewed task SHA-256:** `babc81997278b65e87caecc439062730c24c3ce24d56827fa82496f263b1ea7d` (revision 4)
**Brief lint:** zero findings
**Raw result:** `/private/tmp/a2a-bridge-slice2b2a-spec-review4-20260924/result.md` (SHA-256
`d4b3dd18ed1112bc90a80f58d54e5bf571ff4c15166d15a214816fa82b702466`). The node completed with a clean terminal and
complete cleanup.
**Verdict:** `REJECT`. Round-3 W1 is UNRESOLVED. New: WRONG 1 / SMELL 2; BLOCKER 1 / DEFER 2.

## WRONG

1. **Round-3 W1 is unresolved.** Revision 4's HL3 extension stretches "privileged installation" to cover an in-scope
   operator error.
   - **Scenario:** an operator mistakenly configures a root-owned, non-writable wrapper as Git. It passes the route
     rule, the byte-identical fixed point, and the version check, then dispatches to an unadmitted program. That
     program can fabricate output or effects while the evidence names the wrapper.
   - **Why it is in scope:** the owner ruling excludes only deliberate check-to-use racers and keeps operator error in
     scope.
   - **Offered repairs:** an independently pinned expected digest per admitted route, checked before first execution;
     or an explicit owner amendment excluding wrong configured-root selection from "operator error".

## SMELL

1. The task status line still says round 3 reviews revision 4.
2. The first revision token in the planning handoff sentence says revision 3.

## Classification

The same finding recurred (round-3 W1 → round-4 W1). It is now a threat-model boundary question rather than a spec
defect, so it is escalated to the owner instead of receiving another fold-and-review loop.

## Related host evidence

Probe P7 (does `xcrun --find git` execute the selected Git?) was attempted twice on 2026-09-24. Both attempts are
**inadmissible**, because xcrun rejected the fake developer directory before selecting anything ("missing xcrun at",
then "malformed developer path"). They also showed that `/usr/bin/xcrun` itself dispatches through developer-directory
state. That supports removing the `xcrun` locator altogether.
