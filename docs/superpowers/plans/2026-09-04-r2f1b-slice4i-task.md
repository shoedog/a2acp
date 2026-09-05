---
task-type: implement
---

# R2f1b slice 4I — issue #22 bounded terminalization closure

## Status and custody

**APPROVED / CURRENT-TARGET INTEGRATED LOCALLY / AGGREGATE VERIFIED / PENDING PUBLICATION / NOT MERGED.** Before implementation, live
`origin/main` was rebound to `936534d8cffb225249a5eeccd5874552dc97e961`, and the worktree census found no
competing 4I implementation owner. The owner later renewed the parked artifact for exactly one narrow
interval-endpoint/union repair and one final hard-read-only cumulative rereview, raising the exact-base cap to
**420 added nonblank formatted Rust lines** and authorizing no other expansion. Isolated branch
`feat/r2f1b-4i-terminalization-20260905` retains the complete existing artifact and now holds clean code checkpoint
`59896688f350fa6413740a2254ff0a4d610ece33`, tree `e81f0256cec386a444fc56d282d4c36beeba2fde`, based exactly on that
frozen implementation base. Final Astra review bound exact docs-inclusive candidate
`0132e6bdb8724b29013b5fc2f740bc83c3cba21d`, tree
`5da71fcb9d2fe7083246c033d884a4eb07663fec`, executor blob
`7c59d597ed5c80382bef6a2c4c3ce81e23ed06be`, and returned **APPROVE — 0 WRONG / 1 SMELL-DEFER**. The Rust delta
measures **418 / 420** in `crates/bridge-workflow/src/executor.rs` only. No push, pull request, merge, provider turn,
registry/image effect, compatibility execution, live smoke, release, deployment, running-operator mutation, or 4J
arming occurred. Public `origin/main` subsequently advanced through compatibility/runbook PR #98 to exact
`636979e27eee428981712c506435e0e151ee80a1`, with parents the frozen implementation base and reviewed PR #98 head
`91606a956284447d8fad83eef78f99c3675650ba`; that merge neither contained nor discharged 4I. The owner then
authorized current-target integration and aggregate verification. The approved delta was composed without conflict
onto exact `636979e27eee428981712c506435e0e151ee80a1` as local integration commit
`7169948a3d150694c2f367c53f7c6ce6ce0c4041`, tree
`b11d37e35357182e3444a3859a34d1c3cc722448`, on branch `integrate/r2f1b-4i-current-20260905`. Its executor blob
remains exact `7c59d597ed5c80382bef6a2c4c3ce81e23ed06be`; normalized `git diff` output for both the frozen-base 4I delta
and current-target integration delta has SHA-256
`6da5b5a3c1528731534cc5228c63e515485e570689499a550784d97e0d07c8f3`. The original 4I and integration branches
remain local and unpushed. Approval does not authorize publication, merge, 4J, or any provider/operator effect.

### Measured implementation evidence — 2026-09-05

The required pre-production RED ran through `WorkflowExecutor::run_with_diagnostic_context`, the real scheduler
mux, a frozen automatic fail-fast run, a durable trigger barrier, per-node cancellation, a real non-cloneable
cleanup guard, and successful transfer while the sibling stayed pending. It failed **0 passed / 1 failed / 164
filtered** with `workflow stayed pending after successful cleanup-deadline transfer: Elapsed(())`. The pre-boundary
real-terminal negative control passed **1 / 0 / 164 filtered** on the same production bytes.

The initial repair gave each active node one post-transfer terminalizer, selected a ready real completion first, and
only
signals the synthetic terminal after every materialized cleanup owner for that node has produced an admissible
transfer-derived duration. Review then demonstrated that the transferred guard was dropped before terminalizer
signaling and that workflow cleanup aggregation ignored the transferred interval. Two exact production-path REDs
failed at **0 / 1 / 165 filtered**: custody was absent at workflow settlement after node-future removal, and the
workflow emitted `CleanupObserved Complete/0` instead of conservative `Unknown/60000`.

The first bounded repair retains each exact non-cloneable transfer guard through workflow projection and marks
transferred cleanup `Unknown` while preserving `Failed` precedence. The strengthened fixture asserts custody after
future removal, the aggregate event, complete per-node uniqueness, exactly one workflow terminal, and real-completion
precedence both immediately before and simultaneously at the transfer boundary.

The owner-renewed RED adds a deterministic failed-root teardown interval `[0,1000]`, then lets sibling cancellation
run from `1000` until successful transfer at `61000`. On the parked production code, the root terminal remained
byte-stable and reported `Complete/1000`, the sibling reported `UnknownLegacy/60000`, but the workflow incorrectly
reported `Unknown/60000`; the exact focused command failed **0 / 1 / 165 filtered**. Its exact-boundary negative
control passed **1 / 0 / 165 filtered**. The repair removes duration-only overlays and records the scheduler's exact
`(anchor, now)` endpoints through `WorkflowCleanupTracker::record_interval`; both same-node and workflow projection
therefore use the existing interval-union and disposition logic. The repaired regression passes **1 / 0 / 165
filtered**, the mux passes **14 / 0 / 152 filtered**, and the existing same-node overlapping/disjoint interval-union
control passes **1 / 0 / 165 filtered**. A duration-space mutation `(0, now-anchor)` reproduced the exact
`Unknown/60000` failure at **0 / 1 / 165 filtered**; restoration returned green. In this fixture, the one active
prompt attempt has not completed its warm-turn teardown before transfer. Node-future lifetime alone does not prove
that general exclusion because preflight and retry paths can contribute earlier same-node cleanup intervals; the
node-keyed tracker unions them. No policy, observable bound, manifest, lockfile, readiness value, guard custody,
terminal ownership, or replay behavior changed.

Format and diff are clean. Locked workspace check, warnings-denied locked all-target/all-feature Clippy, locked
all-target/all-feature build, release-bin build, and candidate-built hygiene are green; hygiene reports **41 tracked
artifacts / 9 validated example configs**. A detached trusted-root checkout of exact `59896688` passed the complete
serialized all-target suite at **86 summaries / 4,386 passed / 0 failed / 13 ignored / 0 measured / 714 filtered**;
the separate doctest surface passed **16 summaries / 2 / 0**. Combined totals are **102 summaries / 4,388 passed / 0
failed / 13 ignored / 0 measured / 714 filtered**. All-target and doctest logs have SHA-256
`5076e46d434ff4abac8e8ecb806751f5772529b836c5d7bb5611b412350346aa` and
`60f53dc067d52084f49fb77452e37d94ee3ab047c97b36ab3314614d3edcc7d8`. The 13 ignores remain explicit
authenticated/live ACP-provider, local Ollama, and Docker-image cases; no provider turn ran.

The final reviewer independently ran four focused tests: **4 passed / 0 failed**. It recomputed the retained
all-target and doctest log hashes and totals as **102 groups / 4,388 passed / 0 failed / 13 ignored / 714 filtered**.
Static, build, release, formatting, diff, and hygiene results remain implementation evidence. The sole `SMELL-DEFER`
was the overbroad same-node documentation claim narrowed above; no Rust change was requested or made. The
[final review record](../reviews/2026-09-05-r2f1b-slice4i-astra-final-rereview.md) retains exact provenance and
adjudication.

The separately authorized current-target aggregate ran from detached exact integration commit `7169948a`. Format
and diff checks, locked workspace check, warnings-denied locked all-target/all-feature Clippy, locked
all-target/all-feature build, release-bin build, and candidate-built hygiene **41 / 9** are green. The serialized
all-target suite passed **86 summaries / 4,390 passed / 0 failed / 13 ignored / 714 filtered**; doctests passed **16
summaries / 2 / 0**; combined totals are **102 summaries / 4,392 passed / 0 failed / 13 ignored / 714 filtered**.
Logs are retained privately at `/private/tmp/a2a-r2f1b-4i-integrated.9GzeTJ/workspace-tests.log` and
`/private/tmp/a2a-r2f1b-4i-integrated.9GzeTJ/doctests.log`, SHA-256
`c54a3438476e02f914b28c4e04e18333e2d3cad864a8add7f2b7b90c3df35885` and
`eabd59f763c606d75b313ad88f67242bb4c37b4dd9a6ff68a842a36d40714fcb`. No ignored live-provider test was forced.

## Original concrete residual — terminalization fixed

`WRONG`: use an active frozen attempt with a fail-fast fan-out, two root nodes, and a synthesis node depending on
both roots. Let one root reach a failed terminal and pass the durable trigger barrier. Let the sibling observe its
cancellation but keep its node future pending after its exact cleanup guard has transferred to a recovery owner.
The pre-implementation executor recorded the cleanup transfer and continued to retain that future in
`FuturesUnordered`; its normal exit condition required the set to empty. The incorrect result was no terminal for the sibling, no
`NotStartedPolicy` terminal for synthesis, and no failed workflow terminal. The required result is bounded,
single-shot terminal projection after cleanup custody transfers, without losing the failed root's deepest safe
cause or replaying provider work.

The repaired implementation closes that terminalization wedge and retains cleanup custody through final projection.

## Reviewed WRONG — fixed and independently approved

The prior final rereview proved that `max(previous_cleanup_ms, transferred_duration)` violates the established
shared-clock interval-union contract. The renewed repair implements the reviewer's bounded conceptual correction:
successful transfer settlement records `[scheduler_anchor, current_scheduler_time]` in the existing tracker and
removes both duration-only projections. The production-path RED and mutation above discriminate the exact defect.
The final independent cumulative rereview accepted the correction with no `WRONG` finding.

## Scope and cap

Owned production path:

- `crates/bridge-workflow/src/executor.rs`

Test support may remain in that file or use one narrowly named `bridge-workflow` integration test when only public
interfaces are sufficient. Update this task, the current 4H-2 handoff, and the reliability roadmap only for measured
custody and gate results. Do not change manifests, `Cargo.lock`, fan-out policy values, observable liveness bounds,
readiness, or unrelated cleanup code.

The owner-renewed planning cap is **420 added nonblank formatted Rust lines** against exact implementation base
`936534d8cffb225249a5eeccd5874552dc97e961`. It is a stop boundary; current formatted count is **418 / 420**.

## Required current-base RED

Before production editing, add one deterministic test through `WorkflowExecutor::run_with_diagnostic_context` or
its exact production delegation path. It must use the frozen automatic controls and actual scheduler mux, fan-out
controller, durable trigger barrier, per-node cancellation, cleanup-deadline transfer, node-terminal projection,
and workflow-terminal projection. A helper-only `FuturesUnordered` test or source-text assertion is inadmissible.

Use one shared controllable monotonic clock and paused Tokio time; do not wait the real 60-second cleanup tail.
The sibling backend must expose `ProtectedV3`, attach the exact session owner, retain a non-cloneable cleanup guard,
and return a successful `CleanupDeadlineTransferV1` while its node future remains pending. On the unmodified base,
advance through the exact cleanup deadline and require the test to fail because the workflow future remains pending
and terminal cardinality is incomplete. Retain the complete failing assertion output before editing production.

## Required GREEN and negative paths

The repaired production path must prove all of the following:

1. The failed root terminal and deepest safe cause remain byte-stable and appear exactly once.
2. Policy cancellation reaches the nonterminating sibling once; cleanup transfer completes before the executor
   relinquishes that node future or its sole cleanup owner.
3. The sibling receives exactly one `CanceledPolicy` terminal with an honest non-complete cleanup disposition
   derived from the transferred state; synthesis receives exactly one `NotStartedPolicy` terminal; the workflow
   emits exactly one failed terminal.
4. A sibling that settles immediately before the cleanup boundary keeps its real terminal and is not synthesized a
   second time.
5. A completion that becomes observable after transfer cannot overwrite or duplicate the accepted terminal.
6. Workflow/external cancellation winning first retains `CanceledWorkflow`; policy-first remains
   `CanceledPolicy`.
7. `BoundedIndependent` does not acquire fail-fast behavior: it remains independent until the absolute cutoff,
   then obtains the same bounded post-transfer terminalization.
8. The production refusal gate still proves `scheduler_activation_readiness_v1() == Disarmed` and no production
   caller can construct automatic activation before 4J.

Tests must assert node-terminal cardinality, cleanup disposition, workflow outcome, and retained cause—not merely
task completion or exit status.

## Mechanism constraints

- Never drop a node future while it is the sole cleanup owner. Transfer and retain exact cleanup custody first.
- The post-transfer terminal decision is single-owner and first-write-wins against a concurrently ready real
  completion.
- Do not retry, resume, respawn, or replay the failed or pending provider attempt.
- Reconstruct cleanup settlement from the current monotonic observation; do not reuse a stale pending settlement.
- Preserve the existing mux priority, ready-batch `NodeId` ordering, durable barrier ordering, first-cause
  cancellation mapping, and missing-node synthesis rules.
- Do not arm production. Any required readiness change belongs only to separately authorized 4J.

## Gates and convergence

The implementation/review cap was renewed once for the narrow interval-union repair and one final rereview. Both are
now consumed. No further Rust edit or review is authorized or needed. Current-target integration and aggregate
verification are complete; publication remains a separate authority boundary.

Before presenting the implementation as complete, run and record:

- the exact current-base RED and repaired GREEN, including every negative/edge path above;
- a frozen production mutation that makes the intended regression fail while remaining Clippy-clean;
- `cargo fmt --all -- --check` and `git diff --check`;
- warnings-denied locked all-target/all-feature Clippy and locked build;
- candidate-built `validate --repo-hygiene`;
- the complete serialized workspace suite with exact passed/failed/ignored totals and all exclusions;
- the first hard-read-only review and, after its sole bounded repair, one final cumulative rereview of the exact
  clean candidate.

Any cap breach, failure to establish the production-path RED, attributed non-green gate, custody mismatch,
open-class review population, or required change outside this boundary parks 4I. 4J, R2f2, R3, live compatibility,
release, deployment, and running-operator mutation remain unauthorized.
