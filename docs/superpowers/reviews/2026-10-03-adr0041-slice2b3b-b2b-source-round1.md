# ADR-0041 2B3b B2b — source round 1/2

Raw SHA256 a099b6f6df5b6a4cad6acf46e66f4911dcd4bfb0a06e5d2c8736b4dd7ab1e08b. Actual Sol hard-read-only review; supplied tests were not reviewer executions.
Round1 rejected7d4dc87d/2WRONG1SMELL2blockers. Round2 approved0dadd17e/0WRONG2SMELL0blockers; exact adoption18ff5bea.
Full six-gate raw receipts for both sources mirrored and independently checked AFTER review; reviewer saw bridge PASS summaries only.
Final5149/0/13alltargets,5160/0/13default/16docs,287/0/0focused; all238 inherited names retained.
Process chronology deviation remains disclosed; no restart or round reset. Source-only B2b approval; B2c/native/whole-G pending.
Complete second custody in scratch/b2b-worker-completed-custody/, with original transcripts/checkpoint and all raw trial/final receipts.

---

Bound to clean branch `implement/impl-68504-mnvnlojn`, HEAD `7d4dc87d25829f99957923fa1f0816bb0ab4aedd`, base and merge-base `1e6674ec0763168869b0c2a6832d0d3642f8cf8d`. The owned diff contains only the three declared B2b paths.

### WRONG 1 — The second decoder can exit before validating the complete frame

[The B2b loop](/Users/wesleyjinks/code/.a2a-implement/impl-68504-mnvnlojn/crates/bridge-core/src/custody_restore_git.rs:6644) immediately propagates admission and materialization errors. Its trailer, complete-stream digest, and plan-summary checks occur only after the loop at [line 6667](/Users/wesleyjinks/code/.a2a-implement/impl-68504-mnvnlojn/crates/bridge-core/src/custody_restore_git.rs:6667).

Concrete failure: after the pre-decode rehash, replace a frame with a same-length frame whose first changed entry violates the allowlist and whose remaining stream has a malformed trailer. The existing `frame-rewind` test seam makes this deterministic. B2b returns the early `ActiveFrame` fault without consuming the remaining content, validating the trailer, or detecting the changed frame digest. The required result is a fatal complete-stream/integrity refusal while preserving any partial destination bytes.

B1a already demonstrates the required mechanism: it retains the first admission fault, drains content, validates the complete decoded digest, and only then reports the retained fault at [lines 3302–3336](/Users/wesleyjinks/code/.a2a-implement/impl-68504-mnvnlojn/crates/bridge-core/src/custody_restore_git.rs:3302). This behavior is explicitly required by the B2b task.

Single-operator likelihood is low, but the race is reachable through the existing fault seam and lies inside the custody threat model. Exposure is high because malformed or changed source bytes receive the wrong classification and incomplete validation. The bounded repair is to retain the first non-timeout admission/materialization fault, stop further destination creation, drain the current content and remaining decoder through its trailer, perform the full digest/summary checks, then return the retained fault if integrity remains valid. Add combined regressions for early admission/materialization failure followed by trailer corruption or same-length drift.

**Blocker. Confidence: 99/100.** A direct combined regression would raise it to 100. It would fall if decoder errors were proven unable to follow an entry fault. It collapses only if “drain every full decoder through its trailer” excludes refusal paths, contrary to the task and B1a’s preserved mechanism.

### WRONG 2 — Ref integrity and parsing observe different byte streams

[`check_refs`](/Users/wesleyjinks/code/.a2a-implement/impl-68504-mnvnlojn/crates/bridge-core/src/custody_restore_git.rs:6705) hashes each descriptor, rewinds it, and then parses it in a separate pass. There is no identity or content check between the hash and parser reads. The final gate checks descriptor identity and length, not content digest, at [lines 6317–6330](/Users/wesleyjinks/code/.a2a-implement/impl-68504-mnvnlojn/crates/bridge-core/src/custody_restore_git.rs:6317).

Concrete failure: after the first digest succeeds, change a same-inode, same-length `FETCH_HEAD` note or reflog message before parsing. The parser can accept the changed bytes and produce the same target agreement. B2b then creates `config` and returns `FinalMetadataV1`, although the retained file no longer matches its recorded SHA-256. A malformed replacement instead produces a refs-content error, contradicting the stated integrity-first classification.

This is reachable by a local concurrent writer or a deterministic hook inserted between the two reads. B2c’s later final digest would refuse whole-G success, but it does not make B2b’s internally inconsistent success or premature config effect correct.

The bounded repair is to hash the exact bounded stream consumed by `RefsBuilderV1`, retaining parser faults until the complete stream has been drained and its digest checked. Add same-length mutations after the current prehash: one semantically equivalent change that must prevent config creation and one malformed change proving integrity outranks content.

**Blocker. Confidence: 96/100.** A deterministic boundary-hook regression would raise confidence. It would fall if `FinalMetadataV1` explicitly permitted descriptor contents to disagree with its retained hashes. It collapses only if mutation between retained-descriptor reads is mechanically impossible; the implementation provides no such exclusion.

### SMELL 1 — The required tests-first order was not followed

The handoff explicitly records that production preceded the tests at [line 32](/Users/wesleyjinks/code/.a2a-implement/impl-68504-mnvnlojn/docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md:32). The later behavioral mutation work is substantial, but it cannot establish the requested tests-first chronology.

No incorrect runtime result follows from chronology alone, so this is nonblocking. The artifact must not be restarted; retain the disclosure and enforce tests-first ordering for B2c.

**Confidence: 100/100.**

The source-bound worker evidence reports the untouched-base control at **238 passed, 0 failed, 0 ignored**, the final focused population at **284 passed, 0 failed, 0 ignored**, and 108 normally compiled behavioral mutations with restored final hashes. I treated the missing-symbol structural RED only as structural evidence. The prompt reports all six bridge gates passing. The host mirror still contains no `result.json` or `output.log` for this clone and HEAD, so the B2b full-workspace totals, ignored counts, and exclusions were not independently inspectable. I did not rerun any checks.

This verdict covers B2b only. B2c’s wrapper, final inventory/reachability/fsck, final digest chain, native gates, aggregate gates, and whole-2B3b review remain deliberately deferred.

CAP CLASSIFICATION: CLOSED POPULATION

VERDICT: REJECT
SUMMARY: wrong=2 smell=1 blockers=2 round=1/2
