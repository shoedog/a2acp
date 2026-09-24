# ADR-0041 Slice 2B2 spec review — round 3 (owner-approved extension)

**Date:** 2026-09-24
**Round cap:** the two admitted rounds plus the single owner-approved extension are all consumed
**Reviewer lane:** host Codex `gpt-5.6-sol` / `xhigh` / `read-only` via `codex-acp`, through the operator binary at
merge `d2cbf4e0`
**Execution:** `exec-c272ab8012d9d695f9b9757dfde84a4f` / `attempt-8197c214bbede063ebe2c78884f78475`
**Reviewed commit:** `52e474fa52e41afc1bc3b9c6292f6c6b0067ac4c` (tree `b3cb72e58b28597ca6fc37879062fd66aaac3cf8`)
**Reviewed task SHA-256:** `180e1c3635d35dd76907116b66ff88885d7c27fcd10c30f303c289fe0a8b4f6f` (revision 5)
**Brief lint:** zero findings
**Raw result:** `/private/tmp/a2a-bridge-slice2b2-spec-review3-20260924/result.md` (SHA-256
`99676bd2158ad78075cdba781579ed53716668edbe8ff08c9511b1df5b21924d`). The node completed with a clean terminal and
complete cleanup.
**Verdict:** `REJECT`.

- Round-2 items: 6 RESOLVED (W1, W2, W6, S1, S2, S3) / 2 UNRESOLVED (W4, W5) / 1 ACCEPTED-LIMIT (W3).
- New: WRONG 6 / SMELL 2; BLOCKER 6 / DEFER 2.

The review was scoped to revision 4–5 changes under the owner's §2.1 threat-model ruling. It accepted W3 as honest
limit HL1 and confirmed that the fs_custody seams and receipt/accessor designs are implementable. It treated the
macOS binary facts as unverified supplied evidence and discarded one self-inflicted inadmissible search probe.

## WRONG findings (all folded in revision 6)

1. The route rule said "owned by root, **or** not writable", which admits root-owned writable and user-owned
   read-only routes. **Fold:** root-owned **and** `faccessat(W_OK, AT_EACCESS)` denial; the exporter may not run as
   uid 0; control 28 table.
2. No detection covered the final check-to-exec window. **Fold:** binary identity and SHA-256 recheck after every
   child; HL3 narrowed to a replace-and-restore inside one child's window; control 28b.
3. Trampoline detection needed a spawn that control 28 forbade. **Fold:** two-phase discovery and admission with a
   fixed-point check; only the discovery child runs the candidate; control 28a.
4. The literal link count 2 is non-portable. **Fold:** emptiness proven by descriptor enumeration; no link count;
   control 27d positive.
5. Control 27 did not isolate the parent-entry identity guard. **Fold:** controls 27a–27c, one guard each.
6. The ext4 admission had no fail-first control. **Fold:** control 29 classifier table; `fstatfs` terminology.

## SMELL findings (folded)

1. `root_command` now moves an owned `try_clone` duplicate into the closure and returns `Result`.
2. The §6 method count now says four, and the authorized unsafe code is exactly three audited boundaries.

## Convergence classification

The population is closed and enumerable: each finding names a bounded wording or test fix, and none reopens the
owner-ruled race class. It was folded as revision 6, as declared before the round.

WRONG counts across the loop were 7 → 5 → 6 → 6. The counts are flat while each finding shrinks, and the later
findings were mostly defects in text written to fix the previous round. This is recorded for the owner decision in
task §10 and §16. No further round was run.

This review did not authorize implementation, push, merge, cleanup, 2B3 restore, remote/provider effects, or
running-operator mutation.
