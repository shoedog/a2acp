Bound clean checkout `implement/impl-70428-q52t65ks` at `90130ed98451543e3a5cfba952dd8a2ee2dbd144`, base `dc0b45bafd3538c93fe15254bd9f975ad34f4c71`. The diff contains only the two B2c source/test files and implementation handoff. The normative master hash matches `e3edf7fb…`, and the B2b approval artifact matches supplied SHA-256 `377e56c…`. This is source round 2/2.

### WRONG 1 — The inherited absolute-deadline defect remains in every final-child bracket and callback — BLOCKER

The repaired closing gate is paced correctly, but every success-eligible child bracket still explicitly supplies `unpaced`:

- Runner callbacks perform a mount census and every retained-chain check without the deadline at [custody_restore_git.rs:7724](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:7724).
- Each pre-child baseline census and its descriptor/open guards are unpaced at [custody_restore_git.rs:8013](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8013).
- Successful post-child measurement is also unpaced at [custody_restore_git.rs:8093](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8093). `DigestBoundV1::Deadline` protects digest reads, but the surrounding census, opens, metadata calls, and callback chains remain unpaced.

Concrete regression: during a normal single-operator restore, make `FinalCensusStatted` in a pre-child baseline or successful post-child settlement return just after the absolute deadline. The implementation proceeds to listing and later entry operations before a digest or runner guard notices expiry. Likewise, a callback mount census that crosses the deadline is followed by retained-chain filesystem calls. The required result is an immediate timeout before any subsequent operation; the actual result performs additional I/O and only refuses later.

The new slow-return test does not cover this population. It starts recording only after `final-closing` at [custody_restore_git_tests.rs:26936](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git_tests.rs:26936). The other deadline test expires before the baseline starts and asserts only that the child did not spawn; it does not detect post-expiry census calls.

This is realistically reachable on slow or loaded storage and is exposed around every final child. The repair is bounded: use the original deadline closure for the baseline, both callbacks, and successful settlement; retain clock-free bounded measurement only after a failure is already known, preserving primary-plus-reconciliation failures. Add slow-return regressions outside the closing gate before changing production.

Confidence: **99%**. The implementation and handoff expressly identify the unpaced calls. A targeted hook recording the next filesystem operation would raise this to 100%; only an explicit waiver of the task’s “EVERY slow success validation/read/scan/loop” requirement would collapse it.

### SMELL 1 — The mandatory default/doctest gate is red, but its failure does not discriminate a B2c regression — BLOCKER

The exact candidate’s source-bound receipt records `5,195 passed, 1 failed, 13 ignored`; the failing bridge-core target had `1,442 passed, 1 failed`. The failure says the bootstrap succeeded 1,004 ms after the test’s asserted deadline at [output.log:4266](/private/tmp/adr0041-2b3b-20260929/bridge-full-gate-custody/impl-70428-q52t65ks/90130ed98451543e3a5cfba952dd8a2ee2dbd144/test-default-doctests-lfbj292q/output.log:4266).

That observation does not prove a production deadline violation. The unchanged inherited test captures `begun` before capsule construction and phase-V setup, then reports `begun + 10s` as the deadline at [custody_restore_git_tests.rs:6360](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git_tests.rs:6360). Production creates its actual deadline later, after that setup, at [custody_restore_git_tests.rs:3937](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git_tests.rs:3937). If setup took more than roughly one second under suite load, the observed output is identical even though production returned before its real deadline. The same candidate passed this test in the all-targets gate, and no exact-base control was run in the failing environment.

The gate is nevertheless mandatory and red, so approval lacks required evidence. The bounded disposition is to bind the test to the actual `Instant` supplied to production and obtain the required same-environment base control before attributing the failure. Confidence: **100%** that the gate is red; **95%** that its present clock construction makes the failure nondiscriminating.

### SMELL 2 — Round-2 repair repeated the prohibited production-before-RED chronology — NONBLOCKER

The handoff states that production and tests were written in the same turn and the behavioral RED was obtained retrospectively at [implementation-handoff.md:33](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md:33). That directly violates the increment’s enforced tests-first checkpoint.

The retrospective run is still useful source-bound evidence: 25 passed and 11 failed against round-1 production plus test-only seams. It cannot recreate the missing chronological guard, and it did not expose the remaining unpaced bracket/callback population. Preserve the disclosed history and write the targeted regression before repairing WRONG 1; do not restart the artifact. Confidence: **100%**.

The other B2c mechanisms fit the accepted scope: the wrapper retains one capsule, ledger, pins, budget, and caller deadline; runs A→B1a→B1b→B2a→B2b→final proof; refuses Empty refs before repository creation while leaving direct Pass A Empty support intact; handles captured unborn HEAD with empty objects without `rev-list`; roots final children at retained `.git`; uses strict complete inventory, reachability, and fsck streams; preserves all-outcome reconciliation; and keeps destination-root identity last in the closing gate. I found no new public surface, runner vocabulary, dependency, unsafe code, source route, or writes outside the three owned paths.

The supplied focused evidence reports 323 passed, 0 failed, 0 ignored, including all 287 inherited tests. The final mutation matrix reports 174 predicted behavioral flips, five structural-only rows, nine declared non-flips, two disclosed misses, and no invalid rows. Those receipts substantiate the other round-1 repairs but do not cover WRONG 1.

The bridge’s other final-byte gates passed: fmt, workspace clippy, locked build, all-targets tests (`5,185 passed, 0 failed, 13 ignored`), and hygiene. I did not execute tests locally. Native macOS, native ext4, aggregate verification, the combined base251-to-final review, and 2B3c/2B3d remain deliberately later lanes and are not B2c omissions.

CAP CLASSIFICATION: CLOSED POPULATION. The loop is converging: the prior three WRONG and two SMELL findings have narrowed to one finite remaining pacing class and two evidence/process smells. A disclosed targeted extension on this same artifact is warranted for the enumerated pacing sites and gate-test clock; no restart or checkpoint reset is justified.

VERDICT: REJECT
SUMMARY: wrong=1 smell=2 blockers=2 round=2/2

