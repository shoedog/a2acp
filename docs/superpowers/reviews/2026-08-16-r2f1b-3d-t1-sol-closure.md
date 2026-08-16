I’ll review only the committed `c37338dd..e66b9085` change, without running gates or modifying anything. I’ll freeze repository identity first, then read all three requested artifacts completely and trace the production call, persistence, recovery, and projection paths; no Prism/LSP tool is exposed in this session, so I’ll use bounded read-only search.The checkout is clean and exactly at `e66b9085`; the reviewed range contains only the three declared files. I’m now reading the wire type and handoff in full, then I’ll walk the 10,309-line backend in bounded chunks so the review covers unchanged production context as well as the diff.The full-context read has exposed a concurrency seam worth tracing carefully: aborting the observing configure drops `ConfigureAdmission`, which can start rollback while the detached materializer is still between its one atomic sample and custody admission. I’m following both schedules through the map and publication-cell code before classifying it; I’ll also finish the rest of the file and check whether recovery or terminal projection closes the seam.The repair’s atomic flag is real, but I’ve found a schedule where caller departure lands after its sole sample and before custody/add admission. I’m tracing that gap through configure cleanup and the retained map projection now, because the observable failure may be broader than the phase-2 assertion.That path guess was wrong and is inadmissible evidence; it changes no conclusion. I’m locating the actual production call site, then I’ll check whether cancellation can truly drop this future in the live workflow.The atomic sample itself is a defensible linearization point: a drop observed before it yields `Failed`; a drop after a false sample is a committed phase-3 flight, so I do not count the sample-to-call gap alone as a defect. The blocker is downstream: `ConfigureAdmission::Drop` starts cleanup without joining that committed flight, and cleanup can pop the reservation while the flight still needs it.## WRONG findings

### WRONG 1 — BLOCKER: cancellation cleanup can discard the committed flight’s map ownership

A constructible production schedule is:

1. The runner samples the caller as present and commits the flight at [backend.rs:2771](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-worktree/src/backend.rs:2771).
2. During `add_under_custody`, workflow cancellation drops the configure future through the production `tokio::select!` at [executor.rs:2137](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-workflow/src/executor.rs:2137).
3. `ConfigureAdmission::Drop` immediately starts cleanup at [backend.rs:1187](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-worktree/src/backend.rs:1187). Cleanup waits only for the configure count, not the claimed preparation flight.
4. Cleanup pops the `Reserving` map entry at [backend.rs:1688](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-worktree/src/backend.rs:1688).
5. Because the materialization custodian still holds the publication cell, cleanup gets `CellContended`. `retain_refused_entry` deliberately does not reinsert that class at [backend.rs:2561](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-worktree/src/backend.rs:2561).
6. The runner completes `LiveProtected`, but its best-effort map projection silently accepts a missing entry through `_ => {}` at [backend.rs:2792](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-worktree/src/backend.rs:2792).

The observable result is a durable `LiveProtected` custody record and `Settled` preparation record with no mapped owner or retained identities. `preserve_checkout_v1` and workflow settlement then report `NoCheckoutUnderCustody` at [backend.rs:3681](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-worktree/src/backend.rs:3681) and [backend.rs:3786](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-worktree/src/backend.rs:3786). The exact identities required to mint a preservation claim are permanently lost.

A second schedule exists after the false sample but before `WorktreeCustodianV1::enter`: cleanup can acquire the cell and process the reservation before the committed runner later materializes the checkout.

- Trigger: operator/run cancellation during a slow worktree add or immediately before custody entry.
- Likelihood: `plausible`; cancellation is production-reachable, and add latency gives cleanup time to run.
- Exposure/impact: canceled V3 worktree runs; high custody severity—protected checkout retained without an actionable claim or served settlement.
- Fix: make cleanup join the active preparation flight before popping the reservation, or transfer configure/cleanup admission ownership to the committed runner until map projection completes. Medium backend-local change.
- Red regression: deterministically pause mid-add, abort configure, let cleanup reach the contended cell, then finish the flight and assert preservation/settlement succeeds using the retained exact identities—not merely that disk says `LiveProtected`.
- Ruling: `BLOCKER`; this defeats the central non-cancellable ownership contract, and the current phase-3/4 tests do not inspect it.

### WRONG 2 — BLOCKER: terminal-publication failure is silently discarded after caller departure

If the caller drops during materialization and the final preparation journal write then fails—ENOSPC, I/O error, or ambiguous parent sync—the runner constructs `StoreFailure` at [backend.rs:2808](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-worktree/src/backend.rs:2808). Because the oneshot receiver is gone, [backend.rs:2844](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-worktree/src/backend.rs:2844) discards both the send failure and its typed result, then unconditionally removes the active owner.

The durable preparation record remains `BarrierSynced`, exactly as the phase-5 test demonstrates, but this candidate has no production reader or served projection for that record. The workflow therefore observes cancellation/cleanup, not the terminalization failure. This violates the explicit “loud/typed, never silently swallowed” requirement.

- Trigger: caller cancellation followed by terminal journal persistence failure.
- Likelihood: `rare`, but both mechanisms are production-reachable.
- Exposure/impact: the affected V3 run and operator; high reliability impact because a nonterminal durable flight loses its only active owner and diagnostic.
- Fix: retain a joinable completion/debt record owned by the backend and have cleanup/retirement consume and surface it; do not remove the active flight until terminal publication succeeds or ownership is explicitly transferred to recovery. Medium blast radius, overlapping the preceding coordination fix.
- Red regression: abort after add, inject terminal-publication failure, and assert a surviving cleanup/diagnostic path reports `StoreFailure` or retains typed recovery ownership after the receiver disappears.
- Ruling: `BLOCKER`; phase 5 covers only a live receiver and misses the defining detached-runner edge case.

## SMELL findings

### SMELL 1 — DEFER: red-first liveness checks are unbounded

`wait_for_terminal` waits indefinitely at [backend.rs:454](/Users/wesleyjinks/code/.a2a-implement/impl-95834-kqsue52b/crates/bridge-worktree/src/backend.rs:454), while the handoff describes phases 2–4 as “timing out.” A regression that suppresses the notification hangs until an external job timeout instead of producing a bounded behavioral failure.

- Trigger: caller-owned-runner mutation or any regression preventing terminal notification.
- Likelihood: `plausible` during development or CI mutation controls.
- Exposure/impact: CI/review runs; low production severity but potentially expensive stalled gates.
- Fix: wrap each liveness wait in `tokio::time::timeout` and assert the expected phase-specific failure. Trivial, test-only blast radius.
- Red regression: the named caller-owned-runner mutation fails inside the test’s bound.
- Ruling: `DEFER`; it does not alter production behavior and can be folded into the blocker regressions.

## Evidence assessment

The B21 amendment is sound: `Settled {}` has the exact golden and round trip, rejects unknown fields, the production terminal match is exhaustive without a wildcard, and `Transferred` has no production constructor.

The one Release/Acquire sample is itself a valid admission linearization point. I found no schedule where departure linearized before the sample still admits add, nor one where a live caller is refused. A drop after a false sample is correctly a committed phase-3 flight; the defect is that cleanup does not honor that commitment.

The writer otherwise publishes `Open` before custody effects, advances `BarrierSynced` before add, attempts `Settled`/`Failed`, and verifies its own visible `Open` before repairing an ambiguous initial publication. No production timer, `Transferred` producer, V3 `cleanup_failed_add` path, custody-table edge, or V3 routing change was found.

Phase 2 has a real cancellation mechanism and the supplied mutation is behaviorally discriminating. Phases 3–4 prove disk materialization survives caller abort but are false positives for the claimed map/identity retention. Phase 5 proves loud failure only while the caller receiver remains alive.

I reviewed exact clean head `e66b9085` against `c37338dd` and read all three requested artifacts. Per the read-only contract, I did not rerun gates; the reported 4,117/0/13 suite and clean Clippy/fmt remain supplied evidence.

VERDICT: REJECT
SUMMARY: Two blockers remain: cancellation cleanup can orphan a committed flight’s exact custody ownership, and detached terminal-write failures are silently lost.