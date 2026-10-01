# ADR-0041 2B3b pass-A corrective specification review

Controller receipt: Sol round 1/2 APPROVE, wrong=0 smell=0 blockers=0. This approves the corrective specification only. Source remains rejected pending the disclosed same-artifact Sonnet repair and cumulative source review 3/3.

Raw payload SHA-256: `1f32b058913446db7544c89ce99434181c9d3e2a01d048eaef586144c71c44ee`. Execution `exec-8de85e358ad01b90c4d250d86a9421a5`, attempt `attempt-e263a20f56e7aba1ef3164b669397c07`; terminal node completed. Supplied execution evidence remains receipts, not locally executed review evidence.

The unchanged task and all normative review inputs were copied byte-for-byte into the existing quarantine’s `.git/a2a-evidence/2b3b-pass-a-corrective/controller-input/`, with a hash receipt. The transport maps host-only scratch references to those identical mounted files.

## Raw review payload

I’ll bind the preserved checkout, base, proposal, audit, and owned paths first, then read the full master, audit, both rejection rounds, handoff/evidence notes, and the relevant source. I’ll treat all supplied execution results as receipts and keep this strictly read-only. The corrective review cap is two rounds; this is round 1/2.The bindings match exactly: HEAD `3888ef5c…`, base `251289fe…`, audit `7114b718…`, proposal `42ea4dc…`, and master `e9c8d9e1…`. The base-to-source diff touches exactly the five proposed owned paths, with no working-tree changes reported. I’m now checking whether the proposal’s broad wording is actually complete and executable against the master and every recurrent rejection.The audit does enumerate the obvious accounting domains and all six Git command streams, but approval depends on whether its prescribed mechanics fit the existing seams and whether every error path has a valid regression. I’m tracing the actual reservation lifecycle, child wrapper, census, and parser call order now; that is where a broad “all outcomes” clause can still conceal an unimplementable or contradictory requirement.The existing seams support the proposed correction: the runner funnels all post-spawn failures through one `Err` result, retained pins expose bounded descriptor-relative census operations, and the single ledger can be wrapped by a local ownership token without changing runner vocabulary or public APIs. I’m doing the final contradiction pass now, especially around conservative retention, no-spawn release, ignored diagnostic streams, and whether the named RED/mutation evidence actually discriminates each guard.Bound to the exact preserved artifact:

- Source HEAD: `3888ef5c549cda2ab2044145c843e234712fec75`
- Base and merge base: `251289fe3fada54a740be4879fa48e7adeb9f3cb`
- Corrective task SHA-256: `42ea4dc62a6aa4692d0e56dbf2289cc945f5c748880abf72f56a60339248a841`
- Design audit SHA-256: `7114b718c254a724bb968b55c92e3f101e3a3cfa92b31de12592a6cd1896fc9a`
- Master SHA-256: `e9c8d9e1289bcff53991b5fa49ad011727f0db3a0336cf69a2b15d3eaa69d48a`
- Clean worktree; the base-to-source delta is exactly the five proposed owned paths.

Source hashes were also bound for the four Rust files and handoff. Both original rejection files and the controller evidence notes were read in full.

## WRONG

None.

## SMELL

None.

The corrective scope is a closed population rather than an isolated repair of the last two examples:

- The [accounting design](/private/tmp/adr0041-2b3b-20260929/2b3b-pass-a-design-audit-result.md:98) separately enumerates HOME/XDG roots and descendants, known bootstrap entries, unexpected nested subtrees, and every writing and nominally read-only child.
- It assigns ownership for both entry and logical-byte reservations, defines release-once behavior, and preserves the full reservation when census completion or custody cannot be proved.
- It covers pre-spawn refusal, every post-spawn runner-error class, nonzero status, parser refusal, successful outcomes, and reconciliation failure ordering. Empty’s 11 and Captured’s 14 pass-A entries provide exact success controls.
- The prescribed local reservation token fits the existing ledger and retained-pin seams. No runner, dependency, unsafe, public API, exporter behavior, or pass-B change is needed.
- The [output census](/private/tmp/adr0041-2b3b-20260929/2b3b-pass-a-design-audit-result.md:141) closes the proof-bearing grammar for index-pack, verify-pack, inventory, closure, and fsck, including literal LF, complete UTF-8 records, whitespace, duplication, trailing rows, caps, and command-specific exit-status ordering.
- Ignored streams are bounded diagnostics that the [master contract](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:114) does not use as success evidence. Their lengths and digests remain recorded. The corrective task explicitly forbids widening those exceptions.
- Tests must compile against `3888ef5c`, fail behaviorally with an actual child-start marker, cover the table population, and provide per-guard mutations or mechanism-level equivalence proofs. Full final-byte gates, source hashes, attribution controls, and the corrected eight-section handoff are mandatory.
- The extension is disclosed as one targeted repair turn and one cumulative implementation review round 3/3 on this same artifact. Any remaining open-class issue parks at that cap.

There is no contradiction requiring an owner choice. Approval here applies only to the corrective specification; it does not approve an implementation, supplied receipts, pass B, or a whole-2B3b completion claim.

VERDICT: APPROVE  
SUMMARY: wrong=0 smell=0 blockers=0 round=1/2

