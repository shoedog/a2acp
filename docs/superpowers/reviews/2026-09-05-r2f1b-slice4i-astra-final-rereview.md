# R2f1b slice 4I — final owner-renewed Astra cumulative rereview

**Date:** 2026-09-05
**Provenance:** `[INHERITED FROM CONTROLLER; reviewer /root/r2f1b_4i_astra_review, model gpt-6-astra, hard-read-only]`
**Base:** `936534d8cffb225249a5eeccd5874552dc97e961`
**Reviewed candidate:** `0132e6bdb8724b29013b5fc2f740bc83c3cba21d`
**Reviewed tree:** `5da71fcb9d2fe7083246c033d884a4eb07663fec`
**Executor blob:** `7c59d597ed5c80382bef6a2c4c3ce81e23ed06be`
**Cap:** `418 / 420` added nonblank formatted Rust lines

## Verdict

**APPROVE — 0 WRONG / 1 SMELL-DEFER.**

The owner-renewed repair closes the prior remaining `WRONG`. After an admissible transfer settlement, the executor
records exact scheduler `(anchor, now)` endpoints through `WorkflowCleanupTracker::record_interval`. Node and
workflow projection therefore reuse the established shared-clock interval union instead of composing independent
durations with `max`. The deterministic root `[0,1000]` plus sibling `[1000,61000]` case now emits the required
workflow `Unknown/61000`, while transfer custody, `Failed` precedence, single-owner terminalization, real-completion
priority, first-cause mapping, bounded-independent cutoff, no replay, and `Disarmed` readiness remain intact.

## Independent evidence boundary

The reviewer independently ran four focused tests: **4 passed / 0 failed**. It independently recomputed the retained
all-target log SHA-256 `5076e46d434ff4abac8e8ecb806751f5772529b836c5d7bb5611b412350346aa`, doctest log
SHA-256 `60f53dc067d52084f49fb77452e37d94ee3ab047c97b36ab3314614d3edcc7d8`, and totals: **102 groups / 4,388
passed / 0 failed / 13 ignored / 714 filtered**. The warnings-denied Clippy, locked check/build, release-bin build,
formatting, diff, and hygiene **41 / 9** remain implementation evidence; the reviewer did not claim to rerun those
complete gates.

## SMELL-DEFER

The candidate documentation stated too broadly that a scheduler-transferred active node cannot have an earlier
completed cleanup interval. Node-future lifetime alone does not establish that general exclusion because preflight
and retry paths can contribute earlier cleanup. This is a documentation-scope concern, not a demonstrated incorrect
result: the implementation records every admissible scheduler interval in the node-keyed tracker, whose existing
same-node interval union covers such earlier intervals. The docs-only closure narrows the claim to the tested fixture,
where the one active prompt attempt has not completed its warm-turn teardown before transfer. No Rust change is
required or authorized.

## Disposition

4I is **APPROVED / PENDING PUBLICATION**, not merged. The renewed repair and rereview rounds are consumed; no further
Rust edit or review is authorized or needed. Publication, pull request, merge, provider or operator effects, and 4J
activation remain separately unauthorized.
