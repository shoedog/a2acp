# ADR-0041 2B3b pass A — final source review4/4

Controller received the completed hard-read-only Sol review at 2026-10-01T16:58:07Z.
Clean reviewed HEAD `e6d866faa27481d7176fe3537d4a1bcd5fd8c85c`, base251289fe.
Execution `exec-d7bed171a9179a107c32cd6992f98bf6`,
attempt `attempt-5ddd1f362c0a8468d1c3041e7f92a10c`, session28489 ended0,
node completed/cleanup complete. Raw output SHA256 `823aef565bd67217072b3d4aadfd87d3ac6b1d7e88a6ab4839412a7dffeb2b21`.

**Verdict:** REJECT, WRONG3/SMELL0/blockers3, round4/4, CLOSED POPULATION.
Round3's stable-name mutation and unsafe FIFO findings have direct source closure.
Three remaining defects require descriptor/census identity binding, preservation of the original
absolute deadline, and full-stream fsck classification before typed strict-object selection.

**Controller cap classification:** findings are bounded and enumerable, but counts3→2→1→3 are no
longer converging. Two residuals reopen previously audited custody/parser invariants; the deadline
exception was disclosed and never approved. The declared final4/4 cap is exhausted. Implementation
is PARKED for owner authorization of a narrowly targeted same-artifact repair and new review cap.
No fifth turn, restart, source import or passB dispatch has occurred. Original artifacts/caps and
Rejected checkpoint remain intact; the current candidate remains clean and unapproved.

**Concrete next repair, not dispatched:**
1. Compare digest descriptor fstat against the census's full non-atime facts before AND after reading;
   require exact measured bytes. Add a deterministic alternate-inode swap between census and open,
   restoration after open, and a stable-file control; preserve retained no-follow directory custody.
2. Give every digest the original absolute deadline and refuse success after its expiry. Preserve a
   spent primary timeout plus reconciliation timeout using existing ChildAndReconcile. Correct the
   timeout expectation, add final-fsck near-expiry refusal/control, and remove the exception claim.
3. Reject unclassified fsck records and duplicate notices BEFORE selecting StrictObjectCheck;
   add recognized-strict-plus-noise and recognized-strict-plus-duplicate-notice real-child rows.

Proposed ownership remains the same Git module, tests and implementation handoff, with no new
runner vocabulary/dependency/unsafe or master-spec exception. Sonnet5.5 would make targeted edits
on this exact artifact; valid behavioral base RED, negative controls and source-bound mutations are
required before full unexcluded final-byte gates and a new independent cumulative Sol review.
Owner decision is required because the declared final cap has been exhausted without convergence,
not because ordinary reversible implementation needs permission. PassB remains gated on approval.

The following is raw reviewer output (execution evidence is supplied receipts, not reviewer-local runs).

---

I’m treating this as the disclosed cumulative review round 4/4, with no denominator reset and no further round available. I’ll first bind the exact checkout, commits, five-path delta, file hashes, and review artifacts; then I’ll check the round-3 closure before tracing the full pass-A contract. I’ll make no writes or executions beyond permitted Git/source reads, and I’ll treat all supplied runs strictly as receipts.The binding matches exactly: clean `e6d866f`, parent/base `251289f`, retained predecessor `cab8661f`, the expected five changed paths, and every supplied source/spec hash. I’m now reading the controlling documents and prior verdicts in full, then I’ll compare `cab8661f..e6d866f` first so the inherited WRONG and SMELL are resolved before the broader audit.The round-3 finding is indeed the first review point. The repair now records non-atime metadata plus a descriptor-derived digest, and the FIFO helper is process-based. One contract question remains live: the worker explicitly removed the original absolute deadline from the post-child digest. I’m tracing that call path and every return/error combination before deciding whether it is a source defect; the timeout receipt itself is not authority for that exception.The deadline question is now concrete: every child and the pre-child digest share the caller’s deadline, but `settle_read_only` passes `None` to the post-child digest and can return success after that deadline. The test mutation only shows that the existing timeout expectation changes; it does not authorize dropping the deadline. I’m continuing the cumulative audit for any second defect, especially strict fsck classification and descriptor identity during digesting, before fixing the count.The source now closes the round-3 baseline and FIFO defects at the mechanism level. I’m finishing two contract checks that can still change the verdict: the deliberately unbounded post-child digest, and whether `fsck` can misclassify a stream that contains both a recognized strict-object line and unclassified output.The review remains a rejection. The stable-name round-3 mutation cases and the FIFO fixture are repaired, but three bounded correctness defects remain: the digest descriptor is not tied back to the census inode, the post-child digest escapes the absolute deadline, and `fsck` lets a recognized strict row mask later malformed output. I’m formatting the final with the required 4/4 cap record and receipt boundary.Bound to the clean declared checkout: HEAD `e6d866faa27481d7176fe3537d4a1bcd5fd8c85c`, base/parent `251289fe3fada54a740be4879fa48e7adeb9f3cb`, immediate predecessor and retained ref `cab8661fefb52020e6923bb5de5f0b069c8f53db`. The delta contains exactly the five authorized paths, and all supplied source, master, audit, corrective-task, round-3, and round-4 hashes match.

The direct round-3 cases are repaired: stable-name same-inode/length rewrites and metadata-only changes are compared through non-atime facts and digests; runner failures preserve both errors; atime remains excluded; and the FIFO fixture uses a checked process with no new `unsafe`. Two defects remain in that machinery, plus one inherited parser defect.

### WRONG 1 — BLOCKER: the digest is not bound to the inode measured by the census

The census opens and records one file identity at [custody_restore_git.rs:1490](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1490), but `digest_files` later performs a fresh no-follow open and hashes it without comparing that descriptor’s metadata to the recorded `FileFactV1` at [custody_restore_git.rs:1626](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1626).

Constructible state: after final `fsck`, an in-place rewrite leaves `HEAD` with its original inode and length on a coarse timestamp filesystem. The post census therefore matches. Before the digest reopen, a same-user racer temporarily installs another regular file containing the original bytes at `HEAD`; the fresh descriptor hashes those original bytes, and the racer restores the rewritten inode after the open. The digest comparison at [custody_restore_git.rs:1821](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1821) passes. The following gate checks directory and root identities, while the closing rehash covers only admitted object files at [custody_restore_git.rs:1844](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1844). Pass A can consequently return successful `fsck` evidence while `HEAD` contains the child’s changed bytes.

The bounded repair is to `fstat` each digest descriptor before and after hashing, require it to match the measured identity and complete non-atime facts, and require exactly the measured byte count. A deterministic regression should pause between census and digest open, swap in an original-content alternate inode, restore the changed inode after the descriptor opens, and require custody refusal.

Exposure is low in an ordinary isolated run but direct for the expressly in-scope same-user racer or an admitted wrapper with a surviving helper. Repair cost is localized. Confidence: 98/100; the timing lowers it slightly, and it would collapse only if pre-final name replacement were excluded from the threat model.

### WRONG 2 — BLOCKER: the post-child digest escapes the single absolute deadline

Baseline hashing passes the deadline at [custody_restore_git.rs:1691](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1691), but post-child hashing deliberately passes `None` at [custody_restore_git.rs:1817](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1817). The success path then performs no deadline check before returning at [custody_restore_git.rs:1829](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1829).

A final `fsck` that finishes just before expiry can therefore enter an eight-pass-over-pack post digest, cross the absolute deadline, and still produce `VerifiedGitBootstrapV1`. A timed-out child likewise triggers unbounded digest work after the deadline. Mutation R4-M8 and the timeout/no-effect expectation at [custody_restore_git_tests.rs:5584](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git_tests.rs:5584) encode the implementation’s exception; they do not authorize it.

Pass the original deadline to every digest. If it is already spent, preserve the primary timeout beside the reconciliation timeout through the existing `ChildAndReconcile` path. Update the current timeout regression to require that combined refusal, and add a final-`fsck` near-expiry case that cannot return success after expiry.

Exposure rises with large packs or slow storage; repair cost is very small. Confidence: 100/100. It would collapse only under an explicit post-child exception, which the controller expressly withheld.

### WRONG 3 — BLOCKER: a strict `fsck` row masks malformed trailing output

`classify_fsck` records both strict and unclassified rows, but returns `StrictObjectCheck` before examining the unclassified row or duplicate-notice count at [custody_restore_git.rs:609](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:609).

For stderr:

```text
error: object …: zeroPaddedFilemode: …
unclassified trailing output
```

the reachable result is `StrictObjectCheck`, although the whole stream is not classified and the required result is `GitOutput`. A strict row can similarly mask duplicated allowed notices. The existing table covers invalid UTF-8 before a strict row and notice-plus-strict, but not a recognized strict row combined with an unclassified suffix or duplicate notice at [custody_restore_git_tests.rs:5203](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git_tests.rs:5203).

Validate unclassified rows and notice multiplicity before selecting the strict-object error. Add both mixed-stream cases to the existing real-child table. This remains a refusal, but it returns the wrong typed failure and violates the audit’s no-ignored-suffix requirement. Exposure and repair cost are both small. Confidence: 100/100; it would collapse only if malformed suffixes were explicitly ignorable, contrary to the closed audit grammar.

No additional SMELL was found. Entry and logical-byte accounting, paired ownership and release-once behavior, failed-child reconciliation, Empty/Captured totals and offsets, index/verify/inventory/closure grammars, scope boundaries, and exporter visibility changes otherwise have source-level closure.

The supplied receipts report the behavioral base RED, 93/93 final Git tests, 4,959 all-target passes, 4,970 default/doctest passes, eleven round-4 mutation flips, and successful fmt/clippy/build/hygiene. I treated them only as execution receipts. They do not cover the digest-identity race or mixed `fsck` stream, and R4-M8 supports the rejected deadline exception.

All three findings are bounded members of already enumerated invariants, so the population remains closed. This is the disclosed final round 4/4; no fifth review or denominator reset is authorized. The verdict is source-only for pass A. It grants no pass-B, execution, native-platform, combined-diff, final Git-plane, or whole-2B3b completion claim.

VERDICT: REJECT  
SUMMARY: wrong=3 smell=0 blockers=3 round=4/4  
CAP CLASSIFICATION: CLOSED POPULATION

