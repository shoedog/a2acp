I’ll perform the single capped, hard-read-only review against exactly `6033fd34..2e472a09`. I’ll first freeze checkout/diff identity, then read the entire diff and module in numbered chunks, trace production callers/persistence/projections with read-only search, and finally adjudicate all inherited findings and test evidence. I will not edit, build, test, install, use the network, or invoke another provider.Identity is frozen: the checkout is clean on `2e472a097b77…`, and the range contains exactly the three declared commits. One supplied size description does not match Git’s raw diffstat: the module is `+859/−59` (`918` changed lines), with another `+45` in the handoff. I’ll distinguish production churn from test churn before treating that discrepancy as substantive.The full current module is now read. The repaired reopening path does validate the checkpoint before invoking Task A recovery, and its ahead-set rule is syntactically narrow. I’m now tracing actual reachability and the persistence adapter semantics, because several correctness judgments depend on whether an operation can mutate before the wrapper maps an injected/protective result and on whether this module is production-armed at all.The change should be rejected: one crash-recovery correctness blocker remains.

## WRONG findings

1. **WRONG — orphan healing is not restart-idempotent — BLOCKER**

A constructible state exists after admission publishes child ordinal `N` but crashes before checkpoint advance. On reopen, [remote_request_flight.rs:328](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:328) first durably advances the checkpoint to `N+1`, then separately relabels the child at [remote_request_flight.rs:347](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:347).

If the process dies—or the second Task A replacement fails—between those operations, the next reopen sees the orphan at ordinal `N < checkpoint.next_ordinal`. The ahead set is empty, so reopen returns `Ok` while preserving it as `Active`. No authority was ever returned for that child, yet it is now indistinguishable from an issued request instead of being `PreSendFailure`. The existing idempotence test only repeats reopen after an uninterrupted heal ([remote_request_flight.rs:1389](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:1389)); it does not cut the heal itself.

- Trigger: Unix request-journal backend, boundary-4 admission orphan, followed by process termination or storage/namespace failure after checkpoint sync and before child replacement.
- Likelihood: **rare**, because the window is narrow, but realistically reachable in the exact crash-recovery path this task owns.
- Exposure: no current production callers; once Task C/V3 arms the module, affected restarts lose the proof that the request was never issued. Impact is high because downstream recovery must treat it as ambiguous and cannot safely obtain an authority for it.
- Bounded fix: make the child `PreSendFailure` transition first, sync it, then advance the checkpoint; recognize a unique `PreSendFailure` at `next_ordinal` as the resumable intermediate state. Estimated 30–60 lines, confined to `open` and colocated tests.
- Red regression: create the boundary-4 orphan, inject post-effect `IoUnknown` at the checkpoint-healing replacement, drop the failed reopen, reopen again, and require checkpoint `N+1`, child `PreSendFailure`, and byte-idempotence on a third reopen. Current code leaves `Active`.
- Risk-return: **BLOCKER**. Restart-idempotent orphan healing is explicit B2 intent, and the bounded repair belongs here before activation.

## SMELL findings

1. **SMELL — real-adapter injection repair is incomplete — DEFER**

Stage, publish, acknowledgement replacement, and retirement now execute real adapters before injected mapping, with mutation-sensitive assertions. However, admission checkpoint advance still injects `Replace` before calling the adapter at [remote_request_flight.rs:646](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/remote_request_flight.rs:646), and sync injection remains pre-call. No test injects the orphan-healing replacement itself.

- Trigger: future checkpoint or sync adapter regression.
- Likelihood: **plausible** during subsequent Task C integration.
- Exposure/impact: maintainers and future V3 runs receive incomplete fault-boundary assurance; medium verification risk, but no demonstrated production misbehavior today.
- Fix: wrap actual checkpoint-replacement and sync outcomes and add admission/healing side-effect assertions. Roughly 20–40 lines, test-seam only.
- Red regression: inject each boundary and assert the corresponding real filesystem effect occurred; the admission-replacement case should fail with the present pre-call seam.
- **DEFER** because other behavioral tests execute the adapters and no current wrong output is established.

2. **SMELL — initial B2 behaviors lack admissible fail-first provenance — DEFER**

The initial red evidence for acknowledgement, retirement, restart schedules, and sequential throughput consists of missing-API compile errors. Under this review contract, that is not behavioral evidence. The current 19 tests provide useful positive and negative coverage, and the repair findings have genuine behavior reds, but several initial behaviors were never shown failing on executable pre-change code.

- Trigger: relying on the handoff to prove mutation-sensitive regression coverage.
- Likelihood: **plausible** in later maintenance.
- Exposure/impact: reviewers and maintainers; assurance gap rather than a demonstrated user-facing error.
- Fix: run narrowly controlled mutations—accepting a non-Complete disposition, skipping acknowledgement persistence, skipping retirement, or bounding the sequential loop below capacity—and record each selected behavioral failure. Low-to-medium test-only cost.
- Red regression: the existing focused tests must fail behaviorally under each corresponding mutation, with nonzero selection.
- **DEFER** because source inspection establishes the shipped behaviors despite deficient historical red evidence.

3. **SMELL — the durable handoff is stale after the operator completion — DEFER**

The handoff ends with `09a19025` evidence and does not record `2e472a09`, its side-effect reds, exact-head gates, or the operator-authorized cap extension. Exact accounting is:

- `6033fd34..2e472a09`: module `+859/−59`—production `+279/−34`, test region `+580/−25`—plus handoff `+45`.
- `6115c93e..2e472a09`: production `+88/−42` (130 churn, below 150), tests `+279/−29`, handoff `+17`; total churn is 455, 55 above the original 400 limit.
- The completion commit itself is the disclosed `+43/−21`.

- Trigger: a later operator consumes the repository handoff without this review prompt.
- Likelihood: **common** in the declared handoff workflow.
- Exposure/impact: orchestration and audit consumers; medium custody/evidence risk, no runtime effect.
- Fix: append an exact-head completion section with the extension authorization, reds, gates, and corrected accounting. Very low cost.
- Red regression: a repository check requiring the final handoff record to name `HEAD` and match `git diff --numstat` would fail now.
- **DEFER** because the current prompt supplies the missing disclosure and this does not cause the runtime blocker.

## Adjudications

| Prior finding | Status | Judgment |
|---|---|---|
| Round 1: reopen rewrites issued active children | **PARTIAL** | Issued children below the checkpoint are now preserved, but interrupted healing can strand the proven orphan in that same ambiguous class. |
| Round 1: recovery before attempt authorization | **FIXED** | Checkpoint decoding, schema, attempt identity, and digest validation precede Task A recovery. |
| Round 1: permanent mid-retire `Retained` | **ACCEPTED-RESIDUAL** | The underlying Task A state maps protectively at [namespace_transaction.rs:888](/Users/wesleyjinks/code/.a2a-implement/impl-9079-5czgier2/crates/bridge-core/src/namespace_transaction.rs:888), and B2 surfaces it without mutation. Changing it would exceed this lane’s authority. |
| Repair round: prescribed Clippy findings | **FIXED** | The source contains the requested mechanical fixes; the exact-head Clippy result remains supplied rather than rerun. |
| Repair round: injection rider | **PARTIAL** | The four named test scenarios use real adapters, but checkpoint-advance/sync seams and orphan-heal-specific evidence remain incomplete. |

The operator’s below-checkpoint ambiguity adjudication is accepted for genuinely issued children: B2 correctly avoids guessing until Task C adds durable send rows. The flock-EBADF attribution is **unconfirmed rather than contradicted**: the range does not modify that harness and supplied host gates are green, but no same-environment base control or failure artifact was available, so I do not treat the environmental cause as independently proved.

## Evidence assessment

- Frozen identity: clean `2e472a097b7703eb047a6517b96d4600509fa301`; exactly the three declared commits.
- Scope: only the module and handoff changed. Task A files, `Cargo.lock`, `lib.rs`, routes, providers, and persistence consumers are unchanged.
- Production causality: the module is Unix-exported from `bridge-core`, but repository search finds no production constructor or method caller. There is therefore no served projection or currently exposed run; this is foundational correctness for later activation.
- `git diff --check` passes and the module contains no `rustfmt::skip`. No formatter, build, Clippy, or test command was run under the read-only contract.
- Supplied `631/0` library and `4,045/0/13 ignored` workspace totals are corroboration only.
- Confidence: **96/100**. A cut regression at the checkpoint/child healing boundary would raise confidence; an explicit specification allowing a proven never-issued orphan to become permanently `Active` would lower it. The conclusion would collapse only if the two file replacements were shown atomic or backed by a durable recovery marker spanning the gap; the reviewed mechanism provides neither.

VERDICT: REJECT
SUMMARY: One BLOCKER remains: interrupted reopen healing can strand a proven never-issued orphan as active; adapter coverage and durable evidence also remain deferred.