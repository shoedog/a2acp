# ADR-0041 2B3b pass A — approved cumulative review6/6

Controller received completed hardRO Sol6/6, session49293 ended0, cleanup complete.
Execution exec-b5fd123a664fd41f8934399eb63443de; attempt-47b8107d7af4cac2edf1e250ba735a4c.
Clean reviewed source e4e6dac7540ab488711de5db8a53a8e0bcd535b2, cumulative parent251289fe. Raw SHA256 0217fbce08533031798d93b964118030337874a968020d57aff2af2ebfc7e15c.

**APPROVE: WRONG0/SMELL0/blockers0, round6/6, NO FINDINGS.** Same-artifact late-validation deadline
repair closes source5 blocker without weakening original pins/ledger/deadline/descriptor/parser guards.
Final census and all slow success validation honor original Instant before destination-root identity LAST.
Controller imported ONLY the exact reviewed five paths; original Rejected automatic checkpoint remains untouched.
Full normal Cargo alltargets4978/0/13 (90 groups), default/doctests4989/0/13 (106 groups),112 Git tests each,
no exclusions, runtime hashes before==after==live. Final-handoff hygiene PASS. Sonnet policy cutoff and interrupted
worker tests are distinguished from provider-free complete controller verification.
Main20+supplementary5 mutation builds/restores have checked actual raw populations; main control18 cached,
supplementary19 recompiled. R6-M1 incomplete prediction and hook-only/structural limitations remain explicit.
Approval is source-only pass A. B1/B2 active Git plane, native macOS/ext4 and combined review remain outstanding.
Source/receipts/review/transcripts/checkpoint have second copies at scratch/2b3b-pass-a-round6-final-custody/.

---

I’ll perform the authorized round 6/6 as a hard read-only cumulative review. I’ll first bind the checkout, base, current commit, five-path delta, all supplied identities, and the complete late-validation success-path population; then I’ll assess receipts and causal limits. I’m also reading the repository’s required `a2a-bridge-operator` instructions and the prior ADR-0041 memory index before inspecting the implementation.The binding matches the controller’s clean `e4e6dac7` checkout, the exact base, the five owned paths, and every listed source/spec hash. I’m treating the round 6 execution and mutation artifacts only as supplied receipts. Next I’m enumerating the complete document/evidence set and the production return paths before making any correctness judgment.The controlling record is now consistent: rounds 1–5 form a closed sequence, round 5’s sole blocker is the late-validation deadline escape, and the owner authorized exactly this targeted same-artifact repair plus review 6/6. I’m now tracing the current production code in execution order, including every success return and every failure reconciliation branch, before consulting the detailed mutation rows.The current source closes the round-5 mechanism: the same stored `Instant` now gates post-child success, rewinds, closing mount/tree/rehash/chain work, and the guard immediately preceding root identity; the result construction after root identity is infallible. I found no alternate success return around that sequence. I’m now checking the receipt claims against actual selected tests and raw mutation markers, especially the disclosed R6-M1 overlap and the hook-only Stage B limits.The cumulative trace is complete. I’m doing one final line-level pass over the deadline and sole-success-return seams so the verdict cites the exact mechanism and preserves the round-six cap.**WRONG:** None.

**SMELL:** None.

The checkout is clean and bound to HEAD `e4e6dac7540ab488711de5db8a53a8e0bcd535b2`, base `251289fe3fada54a740be4879fa48e7adeb9f3cb`, and predecessor `b3aa6afd58bbeb51d786e15bc14f94c5be789b99`. The five-path delta and every supplied source/spec hash match.

The cumulative source supports approval:

- The round-four closures remain intact: descriptor `fstat` before and after digesting binds every file to complete non-atime census facts and exact bytes; the same original deadline covers baseline and post-child digests; fsck validates the complete stream and notice multiplicity before selecting strict-object errors. See [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1217) and [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1750).
- One `WorkV1.deadline` now governs every success path. A deadline crossed during the last digest read is observed by the subsequent success gate; mount and exact censuses are paced; admitted files are boundedly rehashed and rewound; retained chains are checked; and the last guard runs immediately before destination-root identity. Only infallible result construction follows the closing gate. See [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1419), [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1945), and [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:2023).
- Failed-child reconciliation remains unpaced and cannot reach verified success. Primary and reconciliation errors remain combined, with entry/byte conservation and release-once ownership preserved.
- The round-three metadata/digest closure, no-follow custody, safe FIFO coverage, strict literal-LF/UTF-8 grammars, stream bounds, and Empty11/Captured14 accounting controls remain present. No unsafe, dependency, runner-command, public API, active-materialization, or pass-B widening appears in the cumulative delta.
- The round-six tests derive the guard population from filesystem state, exercise each success-path guard, preserve a failed-final-child negative control, and distinguish structural assertions from runtime evidence. See [custody_restore_git_tests.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git_tests.rs:7283).
- The receipts show five behavioral pristine-`b3aa6afd` failures plus the passing headroom control. Hook-only tests correctly claim no base RED. R6-M1’s incomplete prediction, extra mutation flips, the reused main-control compilation, and supplementary recompilation are disclosed rather than generalized into blanket proof. R4-M8 and the former timeout exception are superseded.

Approval is limited to the reviewed source for pass A. The execution and mutation results are controller receipts; I did not execute them locally. Pass B, native-platform evidence, the final Git-plane result, and whole-2B3b completion remain outside this approval.

VERDICT: APPROVE  
SUMMARY: wrong=0 smell=0 blockers=0 round=6/6  
CAP CLASSIFICATION: NO FINDINGS

