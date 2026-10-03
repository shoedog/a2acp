# ADR-0041 2B3b B2b — source round 2/2

Raw SHA256 377e56c9a9d58255d0a3254479926013cacb5b3885722e7ae1d009a09eccb2ef. Actual Sol hard-read-only review; supplied tests were not reviewer executions.
Round1 rejected7d4dc87d/2WRONG1SMELL2blockers. Round2 approved0dadd17e/0WRONG2SMELL0blockers; exact adoption18ff5bea.
Full six-gate raw receipts for both sources mirrored and independently checked AFTER review; reviewer saw bridge PASS summaries only.
Final5149/0/13alltargets,5160/0/13default/16docs,287/0/0focused; all238 inherited names retained.
Process chronology deviation remains disclosed; no restart or round reset. Source-only B2b approval; B2c/native/whole-G pending.
Complete second custody in scratch/b2b-worker-completed-custody/, with original transcripts/checkpoint and all raw trial/final receipts.

---

Bound clean checkout at HEAD `0dadd17eaf4b07c0729436a22623a2168e297ae7`, direct parent/base `1e6674ec0763168869b0c2a6832d0d3642f8cf8d`; the owned diff contains only the three declared paths. The predecessor runtime hashes match the base tree. The named predecessor commit itself is absent from this clone.

**WRONG:** None.

The two round-1 WRONGs are closed by mechanism:

- Entry/materialization faults are retained while the decoder drains the complete frame; trailer, digest, and plan checks precede returning the held fault. See [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-68504-mnvnlojn/crates/bridge-core/src/custody_restore_git.rs:6663).
- Ref parsing now hashes the same stream being parsed, drains its remainder, and verifies the complete byte count and digest before returning held content faults. Final-descriptor agreement then covers every retained ref source. See [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-68504-mnvnlojn/crates/bridge-core/src/custody_restore_git.rs:6817).

**SMELL 1 — tests-first chronology was not followed.** The handoff records that the production draft preceded the tests. This cannot produce an incorrect runtime result and has no realistic failing regression, so it remains a process SMELL. Reachability is certain in this increment’s history; runtime exposure is none. Retrospective repair would require the prohibited restart, so the bounded response is to enforce tests-first in B2c. Nonblocker. Confidence: 100%.

**SMELL 2 — revision-bound full-gate receipts are unavailable for inspection.** The supplied result reports all six gates passing, but neither specified custody location contains `result.json` or `output.log` for this HEAD. Therefore the current exact pass totals, ignored totals, exclusions, commands, and source-before/after binding remain uninspectable. This affects evidence strength rather than demonstrated behavior. The bounded repair is to finish mirroring the existing bridge receipts and reconcile the handoff; no duplicate suite is warranted. Nonblocker. Confidence: 100%.

The available source-bound worker evidence is internally consistent: untouched base compiled and passed 238/238; the behavioral RED compiled and selected real tests; the repaired focused population compiled and passed 287/287; mutation records disclose misses and non-flips rather than treating them as behavioral evidence; restored and live source hashes agree. I did not run tests locally.

Approval is limited to B2B. The B2c wrapper, final inventory/reachability/fsck, all-outcome reconciliation, native gates, aggregate gates, and combined final review remain deliberately deferred.

CAP CLASSIFICATION: CLOSED POPULATION

VERDICT: APPROVE
SUMMARY: wrong=0 smell=2 blockers=0 round=2/2
