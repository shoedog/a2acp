# B2a source round2/2 — rejected closed deadline continuation population

Exact110681c023739de8d533b42c454fda56e0b4d88a/basebbf4a2bf; actualSolgpt-5.6-sol/xhigh, session01a0ff11-e419-77b3-a135-ab283387ca10. RawSHA256e2462d58432513809daea8ef3cad9287e4450a4821f136fcf32a7eadc182c4a4. All6 exactsource gates inspected:5094/0/13alltargets;5105/0/13default. Supplied execution, no reviewer tests. Original LoopStopped/Rejected checkpoint retained; disclosed converging extension source3/3, SAME artifact/no restart/reset.

WRONG — The copy loops can perform I/O after the absolute deadline. **Blocker; confidence 98/100.** In [custody_restore_git.rs:5636](/Users/wesleyjinks/code/.a2a-implement/impl-9452-ybd5v28a/crates/bridge-core/src/custody_restore_git.rs:5636), the main read retries `Interrupted` without rechecking the deadline; lines 5663–5677 similarly retry an interrupted or short write; lines 5691–5695 repeat the read problem for the length probe.

A realistic failure is a 16-byte reserved chunk whose first slow write returns `Ok(5)` after the deadline. The loop immediately attempts the remaining 11 bytes. Required behavior is a timeout before that second write, retaining and charging exactly the five-byte prefix. Short writes and interruption are reachable with signals, quotas, storage exhaustion, and FUSE or other slow filesystems. The repair is bounded: guard every retry/continuation, release the reserved unwritten remainder when a post-write guard expires, and add slow-short-write and slow-interrupted-read regressions. Confidence would collapse only if these file operations guaranteed complete, non-interrupted I/O, which their contracts do not.

The final matrix exposes the evidence gap: W04’s interrupted-write branch has no behavioral flip, and no row spends the deadline between partial write calls. The existing post-read and post-sync tests therefore do not cover this case.

Revision-bound supplied evidence is otherwise coherent:

- Clean HEAD `110681c…` over exact base `bbf4a2b…`; owned paths only.
- Untouched-base control: 198 passed, 0 failed, 0 ignored after a separate real bridge-core compilation.
- Repaired focused GREEN: 232 passed, 0 failed, 0 ignored.
- Final mutations: 101 variants; 94 behavioral flips, 2 structural-only, 5 disclosed non-flips; source restored exactly.
- Bridge gates: fmt, clippy, build, and hygiene pass; all-targets `5,094/0/13`; default/doctests `5,105/0/13`. These are inspected supplied receipts, not locally executed tests.
- B2b, B2c, native macOS/ext4, aggregate gates, and combined review remain deliberately deferred.

CAP CLASSIFICATION: CLOSED POPULATION — the defect is bounded to the three retry/continuation loops above and admits a targeted same-artifact repair.

VERDICT: REJECT
SUMMARY: wrong=1 smell=0 blockers=1 round=2/2
