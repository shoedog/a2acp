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

## Round 2/2 — artifact `dbf21d13` (revision 2) — REJECT

Two WRONG·MATERIAL blockers, four SMELL·MATERIAL·DEFER. W1, W2, W4, W5, S1, S2 confirmed closed; W3 only partly.
Classification at the cap: **converging** (6 → 2 blockers, smaller, closed population, bounded repairs; one is
the remainder of W3). Controller discloses **one extension round (3/3)** on the same artifact.

| # | Finding | Disposition (revision 3) |
|---|---|---|
| W1 | Restrictive modes applied before gates that must read or traverse the entries | Folded: everything written `0600`/`0700`; a single post-order closing gate (last step, after index and evidence) verifies and digests, then sets and confirms each final mode |
| W2 | 4095-byte symlink success is impossible on macOS | Folded: typed `DestinationUnsupported` refusal, capability-aware tests |
| S1 | Non-Captured/non-Empty states for the five classes undefined | Folded: exactly Captured-with-payload or Empty, else Admission refusal; table tests |
| S2 | Consumed-byte binding not tested per pass | Folded: separate hooks and tests for each pass |
| S3 | Raw-byte and evidence-symlink branches untested | Folded: byte-distinct names, NFC/NFD, non-UTF-8 targets, evidence symlinks |
| S4 | C2 too broad for a two-round cap | Folded: split into C2a (admission/prevalidation/plan, no writes) and C2b (materialization); index and evidence move into C2b, closing gate and modes into C3 |

## Extension round 3 — artifact `9330136d` (revision 3) — REJECT

One WRONG·MATERIAL blocker (the remainder of the gate/mode ordering: a step-ending output-chain recheck after
sealing a `0000` directory), three SMELL·MATERIAL·DEFER. All other earlier findings confirmed closed.
Classification: converging (6 → 2 → 1, each narrower, same single ordering issue; closed population). The
controller folds the reviewer's exact bounded fixes into revision 4 and runs **no further spec round**: the
remaining text is reviewed again against code in the C3 source review. Disclosed to the owner.

| # | Finding | Disposition (revision 4) |
|---|---|---|
| W1 | Terminal gate cannot recheck output chains after sealing restrictive directories | Folded: one integrated terminal gate (census → post-order verify+seal, each entry's sole terminal check → staging/`.git` chains → root identity last); per-step gate rule narrowed to steps before it |
| S1 | `set_exact_mode(&File)` unusable on `PinnedDirectoryV1` (private file) | Folded: `PinnedDirectoryV1::set_exact_mode` + `set_exact_file_mode`, decoy test |
| S2 | Table charge formula undefined | Folded: 2 × path + target + 64 B, checked; path- and target-dominated bound tests |
| S3 | Connector disclosure cannot identify a non-root separate git dir | Folded: root `.git` disposition + full childless-directory list, no invented identity |
