# ADR-0041 2B3b B2a — source approval 3/3

Exact source88acb02eb01d9ebf37ec287089c57e459b4bf866, original basebbf4a2bf8386d6be18bc6e68959c1fce7e7e4cd6.
Actual hard-read-only Solgpt-5.6-sol/xhigh session01a0fffb-f4db-75c1-8292-a87a65b9bc48; workflow85270 completed with cleanup complete.
Raw SHA25682c532acf7bbf1fd30febdf1b5e0722f0771810c4aa98e2af3b484be1395acbb.
Original source1/2 and2/2 rejections remain intact; disclosed converging extension3/3 repairs the same artifact.
Controller adopted only three reviewed owned paths atc8eb29103681df74a35c814203823de8ebb40e56, runtime byte-identical.
Full raw review and transcript retained at scratch/b2a-round3-review-completed-custody/.
Source approval only; B2b/B2c/native/aggregate/combined gates remain pending.

---

No WRONG or SMELL findings.

The clean clone remains bound to HEAD `88acb02eb01d9ebf37ec287089c57e459b4bf866`, parent `110681c023739de8d533b42c454fda56e0b4d88a`, and base `bbf4a2bf8386d6be18bc6e68959c1fce7e7e4cd6`. The owned diff contains only the two Git-plane files and B2a handoff.

The inherited WRONG is closed by mechanism:

- Interrupted reads and writes check the original deadline before retrying ([custody_restore_git.rs:5730](/Users/wesleyjinks/code/.a2a-implement/impl-9452-ybd5v28a/crates/bridge-core/src/custody_restore_git.rs:5730)).
- Each accepted write prefix immediately advances the offset, retained `written` fact, and charge. A short write checks the deadline before continuing, while refusal releases exactly the unwritten remainder ([custody_restore_git.rs:5763](/Users/wesleyjinks/code/.a2a-implement/impl-9452-ybd5v28a/crates/bridge-core/src/custody_restore_git.rs:5763)).
- The length probe is guarded before its first call and every interrupted retry ([custody_restore_git.rs:5804](/Users/wesleyjinks/code/.a2a-implement/impl-9452-ybd5v28a/crates/bridge-core/src/custody_restore_git.rs:5804)).
- The earlier post-read and post-file-sync deadline guards remain intact ([custody_restore_git.rs:5750](/Users/wesleyjinks/code/.a2a-implement/impl-9452-ybd5v28a/crates/bridge-core/src/custody_restore_git.rs:5750), [custody_restore_git.rs:5831](/Users/wesleyjinks/code/.a2a-implement/impl-9452-ybd5v28a/crates/bridge-core/src/custody_restore_git.rs:5831)).

Thus, the prior realistic case—a 16-byte reservation whose slow write accepts five bytes after the deadline—cannot issue another write: the retained fact, ledger charge, and file length all remain exactly five bytes. The targeted regressions exercise slow short writes, interrupted retries, every selected call position, length-probe retries, successful headroom, and later I/O failures ([custody_restore_git_tests.rs:16940](/Users/wesleyjinks/code/.a2a-implement/impl-9452-ybd5v28a/crates/bridge-core/src/custody_restore_git_tests.rs:16940), [custody_restore_git_tests.rs:16997](/Users/wesleyjinks/code/.a2a-implement/impl-9452-ybd5v28a/crates/bridge-core/src/custody_restore_git_tests.rs:16997), [custody_restore_git_tests.rs:17123](/Users/wesleyjinks/code/.a2a-implement/impl-9452-ybd5v28a/crates/bridge-core/src/custody_restore_git_tests.rs:17123), [custody_restore_git_tests.rs:17198](/Users/wesleyjinks/code/.a2a-implement/impl-9452-ybd5v28a/crates/bridge-core/src/custody_restore_git_tests.rs:17198)).

The supplied source-bound evidence is coherent:

- Untouched parent control: 34 passed, 0 failed, 0 ignored, with an actual `bridge-core` compilation.
- Corrected behavioral RED: 36 passed and four relevant failures, with an actual compilation. The earlier invalid target-path trial is excluded.
- Final focused GREEN: 238 passed, 0 failed, 0 ignored.
- Current repair matrix: 15 variants plus two controls; all 15 predicted behavioral flips occurred, every row compiled, every mutation digest was independently reconstructed, and all five source files were restored exactly.
- Controller Linux gates all passed on the committed read-only source: all-targets reported 5,100 passed, 0 failed, 13 ignored; default plus doctests reported 5,111 passed, 0 failed, 13 ignored. The receipt hashes and before/after state match HEAD and the supplied source hashes. These are inspected controller receipts, not tests executed by this reviewer.

This is source approval for B2a only. B2b metadata/ref/config materialization; B2c wrapping, final inventory/reachability/fsck and reconciliation; native macOS/ext4 gates; aggregate review; and whole-2B3b completion remain deliberately deferred.

CAP CLASSIFICATION: NO FINDINGS

VERDICT: APPROVE
SUMMARY: wrong=0 smell=0 blockers=0 round=3/3
