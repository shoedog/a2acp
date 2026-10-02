# ADR-0041 2B3b pass A — owner-approved source review5/5

Controller received completed hard-read-only Sol review at2026-10-01T20:41:00Z.
Clean reviewed HEAD b3aa6afd58bbeb51d786e15bc14f94c5be789b99, cumulative base251289fe.
Execution exec-9ffe056038119b926842bcbd816a15ee,
attempt-287fe43320ec5cce2c97c228595e4a7e, session50065 ended0; node completed/cleanup complete.
Raw output SHA256 `861908430701368bc06f67d028870768960ddddbe16c4b13cf99f45830912ca4`.

**Verdict:** REJECT, WRONG1/SMELL0/blockers1, round5/5, CLOSED POPULATION.
The descriptor/census identity and whole-stream fsck blockers have mechanism-level closure.
Both digest passes use the original deadline, but later success/final validation can still expire it
and return VerifiedGitBootstrapV1. Worker disclosed that limitation; controller never approved it.

**Controller cap classification:** CLOSED, localized residual of the original deadline invariant;
counts3→1 converge for this owner-approved repair. Owner authorized ONE repair plus source review5/5,
not unlimited rounds. That cap is exhausted, so source remains UNAPPROVED and implementation PARKED
for owner decision at that stable point. Owner subsequently replied "another repair is authorized";
one targeted same-artifact Sonnet5.5 repair was dispatched, followed by cumulative source review6/6.
That turn ended at its fixed policy limit during full testing; exact container absent, source/evidence
snapshotted. Provider-free controller verification is running before source6/6; no provider replay.
No review6 verdict, restart, source import or passB occurred. Same preserved artifact,
all rejected refs/checkpoints and complete execution evidence remain preserved.

**Concrete bounded repair, subsequently authorized and dispatched:**
- Carry the same original Instant through the post-child success sweep and closing gate, including
  bounded census/chain loops, staged rewinds where needed and admitted pack/idx/rev streaming rehash.
- Check deadline after potentially slow reads and during loops, then immediately BEFORE the final
  destination-root identity check. Preserve census→exact tree/digest→retained chains→root LAST,
  with only infallible construction after that identity check. No new deadline or final-gate exception.
- Add deterministic real-path expiry at BeforeFinalGate and during closing census/rehash; verify all
  six Git children started and require typed Timeout. Include a with-headroom success control and
  source-bound guard-removal mutations for late-expiry paths. Keep all100 existing Git tests.
- Sonnet5.5 edits ONLY the same Git module/tests/implementation handoff. Same-environment admissible
  base RED and negative controls precede localized corrections; full unexcluded final-byte suites,
  source/hash custody and one new cumulative Sol source review6/6 follow under the owner's authorization.

Round6 task SHA256 `ef45bb94fe02e0d77674079b63e3b29eb5320334f11b033945692e99ddccc966`;
fresh doctor27 checks and Sonnet catalog pass. Mounted inputs bind clean b3aa6afd and all five hashes.
Session56776, execution exec-57b25cef7aa29dd8bb2a2025f8a16dda,
attempt-9e2643c80d4b23ced3b5e20361021e89. Cap6/6 declared before dispatch; no silent seventh round.

Full worker suites remain4966 all-target/4977 defaultdoctests,0failed/13ignored each/no exclusions,
fmt/clippy/build/hygiene PASS and13/13 mutations. These are source-bound execution RECEIPTS, not
reviewer execution and not coverage of this remaining late-validation deadline path.
Native macOS/ext4 and combined review remain after approved A+B. Whole2B3b is incomplete.

---

I’ll conduct the authorized final 5/5 review as a hard read-only pass. I’ll first bind the checkout, parent/base, five-path delta, source hashes, and every named task/review/evidence artifact; then I’ll trace the three round-4 WRONG closures before assessing the earlier census, parser, accounting, and test-causality requirements. I’ll treat all run logs as controller receipts only and will not execute code or tests.The initial binding matches the controller input: the checkout is clean at `b3aa6afd`, its parent is the stated `251289fe` base, the retained round-4 predecessor resolves to `e6d866fa`, and the cumulative delta contains exactly the five authorized paths. I’m now validating every supplied SHA and inventorying the complete review/evidence corpus before reading it in bounded chunks.The prior record is now explicit. Round 4 left exactly three CLOSED POPULATION WRONGs: digest-to-census identity, single-deadline coverage, and whole-stream `fsck` typing. I’m reading the approved corrective contract and final handoff next, then I’ll inspect the production diff beginning with those three mechanisms.The three round-4 repairs are present at the production seam: digest descriptors are fstat-bound before and after reads with exact byte counts; post-child digests now receive the stored original deadline; and `fsck` classifies unrecognized records and duplicate notices before selecting `StrictObjectCheck`. The disclosed deadline gap is also real in source: after the last digest read, both the post-child gate sweep and the closing census/rehash/chain sweep can run past the deadline and still reach success. I’m checking its exact final-path reachability and regression shape while completing the cumulative accounting/parser and test audit.The three round-4 repairs are source-visible, but the deadline repair appears incomplete: the same `Instant` reaches both digest passes, while slow work after the last checked read can still cross it before the root-last gate and return success. I’m finishing the remaining test and receipt audit to determine whether any regression actually closes that path or whether it remains the sole blocker.The regression suite covers the repaired post-child digest timeout and the descriptor races, but its near-expiry test sleeps before settlement. It does not exercise expiry during the final census/rehash/chain sweep, which is the reachable path still lacking a deadline check. I’m binding exact source lines and the receipt limitations now, then I’ll issue the final 5/5 classification.The checkout is clean and matches the supplied binding: HEAD `b3aa6afd58bbeb51d786e15bc14f94c5be789b99`, cumulative base `251289fe3fada54a740be4879fa48e7adeb9f3cb`, and retained predecessor `e6d866faa27481d7176fe3537d4a1bcd5fd8c85c`. The five-path delta and all supplied hashes match.

### WRONG

1. **The original absolute deadline can expire during final validation and still produce `VerifiedGitBootstrapV1`.** Confidence: **99/100**.  
   [custody_restore_git.rs:1912](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1912), [custody_restore_git.rs:1929](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1929), [custody_restore_git.rs:1965](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1965), [custody_restore_git.rs:2528](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:2528)

   **Concrete state:** a Captured run completes the final fsck settlement with little deadline headroom. After the last digest check, scheduler delay, mount enumeration, exact-tree reconciliation, admitted pack/idx/rev rehashing, or retained-chain checks consume the remaining time. None of `settle_read_only`’s success gate, `remeasure_admitted`, `closing_gate`, or `run_steps` checks the original deadline. If those operations otherwise succeed, lines 2530–2536 return successful closure evidence after expiry.

   The near-expiry regression sleeps in the final child’s `AfterChild` callback, before settlement begins, so its post-child digest catches the spent deadline. It does not cover expiry during the subsequent success gate or closing gate: [custody_restore_git_tests.rs:6351](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git_tests.rs:6351).

   **Bounded repair:** carry the same original `Instant` through the success sweep and closing gate; make admitted-file rehashing deadline-aware; check during bounded loops and immediately before the destination-root identity check, preserving root identity as the last fallible gate. Add a deterministic regression that crosses the deadline at `BeforeFinalGate` or during the closing census/rehash, asserts `Timeout`, confirms all six children started, and retains a headroom success control.

   **Confidence evidence:** direct control flow raises confidence; the full source contains no later deadline guard, and the worker handoff independently discloses the omission. Lack of local execution lowers it by one point, although the failure is constructible from source and the existing test hook. Confidence would collapse if a mandatory downstream check compared the original deadline before success construction; none exists.

The other inherited findings are closed at mechanism level:

- Descriptor-relative no-follow opens now compare complete non-atime facts before and after hashing and require the exact measured byte count.
- Whole-stream fsck classification and notice multiplicity precede strict-object selection.
- Round-3 metadata, same-inode mutation, combined refusal, atime, FIFO, and no-`unsafe` closures remain intact.
- Accounting, reservation release, reconciliation, command grammars, output caps, exit ordering, Empty-11/Captured-14 totals, and descriptor-offset controls yielded no further finding.
- R4-M8 and the old bare-timeout expectation are explicitly superseded.

The supplied receipts report fmt, clippy, build, hygiene, 4,966 all-target test passes, 4,977 default/doctest passes, and 13/13 mutation flips. I treated these as execution receipts and performed no local execution. Their test population does not cover the remaining post-digest deadline path.

This verdict covers source-only pass A. Pass B, final Git-plane output, native filesystem validation, combined review, and whole-2B3b completion remain outside this review.

VERDICT: REJECT  
SUMMARY: wrong=1 smell=0 blockers=1 round=5/5  
CAP CLASSIFICATION: CLOSED POPULATION
