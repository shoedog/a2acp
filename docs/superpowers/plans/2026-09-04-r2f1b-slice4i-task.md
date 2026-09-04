---
task-type: implement
---

# R2f1b slice 4I — issue #22 bounded terminalization closure

## Status and custody

**Drafted from a current-main census; implementation and review are not authorized by this document.** The
measured census base is `origin/main` `52b05d70f14fc1080707fde1de4e9818a9d81d0f`, which contains 4H-2 merge
`54529b1d83a9fbe97d400cded02dcfbdf69683e3` from PR #89 and provider refresh PR #90. Fetch and rebind exact
`origin/main` before implementation. Stop if the target moved without an explicit integration decision or another
4I owner exists.

## Concrete residual

`WRONG`: use an active frozen attempt with a fail-fast fan-out, two root nodes, and a synthesis node depending on
both roots. Let one root reach a failed terminal and pass the durable trigger barrier. Let the sibling observe its
cancellation but keep its node future pending after its exact cleanup guard has transferred to a recovery owner.
The current executor records the cleanup transfer and continues to retain that future in `FuturesUnordered`; its
normal exit condition requires the set to empty. The incorrect result is no terminal for the sibling, no
`NotStartedPolicy` terminal for synthesis, and no failed workflow terminal. The required result is bounded,
single-shot terminal projection after cleanup custody transfers, without losing the failed root's deepest safe
cause or replaying provider work.

The alternative adapter-only explanation is ruled out for this scheduler state: issue #22's deterministic fake
backend reproduced the wedge, and current source retains the same empty-`inflight` exit condition after successful
cleanup transfer. The current 4H-2 module passes 11 tests but contains no nonterminating sibling held past transfer.

## Scope and cap

Owned production path:

- `crates/bridge-workflow/src/executor.rs`

Test support may remain in that file or use one narrowly named `bridge-workflow` integration test when only public
interfaces are sufficient. Update this task, the current 4H-2 handoff, and the reliability roadmap only for measured
custody and gate results. Do not change manifests, `Cargo.lock`, fan-out policy values, observable liveness bounds,
readiness, or unrelated cleanup code.

The planning cap remains **400 added nonblank formatted Rust lines**, as assigned by the slice-4 decomposition.
It is a stop boundary. Recount against the exact implementation base after `cargo fmt`; unused capacity from any
other slice is not transferable.

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

The implementation/review cap is **two targeted rounds**: one RED-first implementation plus one hard-read-only
review, then at most one bounded repair plus rereview. At the cap, classify convergence before acting; never restart
the artifact.

Before presenting the implementation as complete, run and record:

- the exact current-base RED and repaired GREEN, including every negative/edge path above;
- a frozen production mutation that makes the intended regression fail while remaining Clippy-clean;
- `cargo fmt --all -- --check` and `git diff --check`;
- warnings-denied locked all-target/all-feature Clippy and locked build;
- candidate-built `validate --repo-hygiene`;
- the complete serialized workspace suite with exact passed/failed/ignored totals and all exclusions;
- one fresh hard-read-only cumulative review of the exact clean candidate.

Any cap breach, failure to establish the production-path RED, attributed non-green gate, custody mismatch,
open-class review population, or required change outside this boundary parks 4I. 4J, R2f2, R3, live compatibility,
release, deployment, and running-operator mutation remain unauthorized.
