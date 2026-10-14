# ADR-0041 Slice 2B3c spec review ledger

Cap: two Sol/xhigh rounds (`code-review` workflow, `gpt-5.6-sol`/xhigh, hard read-only), declared before round 1.

## Round 1/2 — artifact `a9065798` (revision 1) — REJECT

Six WRONG·MATERIAL blockers, three SMELL·MATERIAL·DEFER. Closed population; every finding named an input/state,
the wrong result and a bounded fix. Disposition (all folded in revision 2):

| # | Finding | Disposition |
|---|---|---|
| W1 | Decoded bytes not bound to the staged artifact after rehash | Folded: consumed-byte count+SHA binding in both decode passes, as phase G |
| W2 | Final gate does not re-digest output files | Folded: closing gate reopens each file by name no-follow, checks dev/ino, streams length+SHA |
| W3 | Non-searchable directory modes make the final gate impossible | Folded: post-order closing gate seals each directory's mode after its descendants verify |
| W4 | Ledger overcharges failed partial writes | Folded: release exactly the unwritten part of a chunk |
| W5 | In-process umask test races the test binary | Folded: isolated child process only |
| W6 | Local validation weaker than parent plan §8 | Partly folded: the owner's 2026-10-13 standing rule (impacted tests locally, full suite in CI as merge gate) supersedes §8's local matrix; workspace clippy, repo hygiene and `git diff --check` added |
| S1 | D5 allowed set ambiguous | Folded: exact eight-class union |
| S2 | Prevalidation table unbounded | Folded: 64 MiB bound like phase G, test seam |
| S3 | D3/D6/D7 lack opposite-branch tests | Folded: controls added to C2 |
