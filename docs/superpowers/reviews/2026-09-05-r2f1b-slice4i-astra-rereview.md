# R2f1b slice 4I — final Astra cumulative rereview

**Date:** 2026-09-05
**Provenance:** `[INHERITED FROM CONTROLLER; reviewer /root/r2f1b_4i_astra_review, model gpt-6-astra, hard-read-only]`
**Base:** `936534d8cffb225249a5eeccd5874552dc97e961`
**Reviewed candidate:** `f7917e3acc5128f289681476ec1061b1f1a2fd7a`
**Reviewed tree:** `15cb0a8208679af10a4a048eb36d542afe3511a2`
**Executor blob:** `a462c864ba542c68c163a2726d1ac594181e1853`
**Current status:** historical verdict retained below; owner-renewed exact `0132e6bd` received final
[`APPROVE`](2026-09-05-r2f1b-slice4i-astra-final-rereview.md)

## Verdict

**REVISE — 1 remaining WRONG / 0 SMELL. The final review cap is reached; park 4I.**

## Inherited finding adjudication

- **W1 — FIXED.** `retained_cleanup_transfers` owns each complete transfer, including its non-cloneable guard,
  through node-future removal and final workflow projection.
- **W2 — PARTIAL.** The candidate now projects transferred cleanup as `Unknown` while preserving `Failed`
  precedence, but its duration aggregation is incorrect for disjoint cleanup intervals.
- **S1 — FIXED.** The strengthened fixture asserts three unique node terminals, one workflow terminal, outcome,
  retained cause, and cleanup; real completion wins both immediately before and exactly at the transfer boundary.

## Remaining WRONG

`crates/bridge-workflow/src/executor.rs` in the final workflow projection combines the tracker's existing interval
union and the scheduler transfer duration with:

```rust
cleanup_ms = cleanup_ms.max(ms);
```

That does not preserve the established interval-union contract. A constructible active fail-fast run has the failed
root's teardown occupy shared-clock interval `[0, 1000]`, then starts sibling cancellation at `1000` and transfers
the still-pending sibling at `61000`, contributing interval `[1000, 61000]`. `WorkflowCleanupTracker::observation()`
returns `1000`; scheduler transfer evidence contributes duration `60000`; the candidate emits workflow
`CleanupObserved Unknown/60000`. The correct union of `[0, 1000]` and `[1000, 61000]` is `61000`.

The same duration-only `max` composition in node projection can undercount an earlier disjoint cleanup interval for
that node. A bounded conceptual correction would retain scheduler interval endpoints in the shared monotonic-clock
domain and include them in interval union. That correction is not authorized: the implementation used its sole
repair round and independently recounts at exactly **400 / 400 added nonblank formatted Rust lines**.

Production exposure remains fenced because `scheduler_activation_readiness_v1()` is `Disarmed`. That reduces live
reachability; it does not make the candidate's output correct or permit approval.

## Other cumulative checks

The reviewer found no new regression in single node-future ownership, biased real-completion priority, first-cause
mapping, `BoundedIndependent` cutoff, no-replay behavior, or the `Disarmed` readiness fence. The reviewer
independently recounted the Rust cap at **400 / 400**.

The reviewer did not freshly build or test. Every retained executable under `/private/tmp/*4i*target*` was stale and
listed the superseded `real_sibling_terminal_just_before_transfer_boundary_wins` test. The repaired implementation
binary retained outside `/private/tmp` at
`/Users/wesleyjinks/code/a2a-r2f1b-4i-gate-20260905/target/debug/deps/bridge_workflow-ecd7967c64cd8321`
lists `real_sibling_terminal_at_or_before_transfer_boundary_wins`; its SHA-256 is
`e188d0701bee38a605ac30bc81c8b4dda73c1a50c39cec74e302f570781f132f`. No final-repair file log was retained, so
the implementation's **4,388 passed / 0 failed / 13 ignored** combined totals remain supplied implementation
evidence, not independently reproduced review evidence.

## Stop disposition

4I is **PARKED / REVISE** at the declared final review cap. Do not edit Rust, rerun a repair loop, publish, merge,
arm 4J, or contact providers under this lane. Any renewed implementation requires a new owner decision with an
explicit cap and a bounded interval-endpoint/union contract.

## Post-review owner-renewed closure candidate

**This section is implementation custody, not a reviewer verdict.** After the verdict above, the owner explicitly
renewed the existing artifact for exactly one narrowly bounded interval-endpoint/union repair and one final distinct
hard-read-only cumulative rereview. The exact-base cap became **420 added nonblank formatted Rust lines**; no other
scope, publication, provider, operator, or activation authority changed.

Clean repair-code checkpoint `59896688f350fa6413740a2254ff0a4d610ece33`, tree
`e81f0256cec386a444fc56d282d4c36beeba2fde`, executor blob
`7c59d597ed5c80382bef6a2c4c3ce81e23ed06be`, remains linear from exact base
`936534d8cffb225249a5eeccd5874552dc97e961` and measures **418 / 420** added nonblank formatted Rust lines. It replaces
duration-only projection with the reviewer's bounded conceptual correction: after admissible transfer settlement,
the executor records exact scheduler `(anchor, now)` endpoints through `WorkflowCleanupTracker::record_interval`.
The existing tracker performs shared-clock interval union for both node and workflow projections and preserves
`Failed` precedence over transferred `Unknown`. Transfer-guard retention and all previously fixed arbitration,
terminal-cardinality, cause, cutoff, and no-replay mechanisms remain unchanged.

The deterministic production-path RED constructed root teardown `[0,1000]` followed by sibling transfer
`[1000,61000]`; parked production code emitted actual workflow `Unknown/60000` and failed **0 / 1 / 165 filtered**
against required `Unknown/61000`. Repaired primary and mux gates pass **1 / 0 / 165 filtered** and **14 / 0 / 152
filtered**. Replacing exact endpoints with duration-space `(0, now-anchor)` reproduced **0 / 1 / 165 filtered**;
restoration returned green. The existing tracker control covers overlapping plus same-node disjoint interval union at
**1 / 0 / 165 filtered**. In this tested fixture, the one active prompt attempt has not completed its warm-turn
teardown before transfer. Do not generalize that fixture fact from node-future lifetime alone: preflight and retry
paths can contribute an earlier same-node cleanup interval, which the node-keyed tracker must union.

Exact trusted-root all-target evidence at `59896688` is **86 summaries / 4,386 passed / 0 failed / 13 ignored / 714
filtered**; doctests are **16 summaries / 2 / 0**. Their retained logs have SHA-256
`5076e46d434ff4abac8e8ecb806751f5772529b836c5d7bb5611b412350346aa` and
`60f53dc067d52084f49fb77452e37d94ee3ab047c97b36ab3314614d3edcc7d8`. Format, diff, locked workspace check,
warnings-denied locked all-target/all-feature Clippy, locked all-target/all-feature build, release-bin build, and
candidate-built hygiene **41 / 9** are green. `scheduler_activation_readiness_v1()` remains `Disarmed`; no provider,
registry/image, compatibility, live smoke, release, deployment, publication, merge, or running-operator effect ran.

The renewed repair and final rereview are consumed. Independent Astra review of exact docs-inclusive candidate
`0132e6bdb8724b29013b5fc2f740bc83c3cba21d`, tree `5da71fcb9d2fe7083246c033d884a4eb07663fec`, returned **APPROVE — 0
WRONG / 1 SMELL-DEFER**. The deferred documentation qualification is reconciled above and in the
[final review record](2026-09-05-r2f1b-slice4i-astra-final-rereview.md). 4I is approved pending separately authorized
publication; no further Rust edit or review is authorized or needed.
