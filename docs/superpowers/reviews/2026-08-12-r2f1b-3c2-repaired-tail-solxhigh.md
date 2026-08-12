# R2f1b 3c2 repaired-tail review — Sol/xhigh

Date: 2026-08-12.
Artifact: `feat/r2f1b-3c2-api-authority` @
`cecff376e4e3c5b705d83cf21f402203ae2a9583`; review parent
`772518a8aaeaa6c52a2bbd445123fc945aeb8056`; original slice base
`42249b3d926b49afd9d0dbd213d0ee3d3e459af6`.

## Dispatch identity

One repaired-tail review round was declared. The candidate release binary
validated the scratch config; `doctor --json` was entirely OK; and `models`
confirmed `gpt-5.6-sol[xhigh]` with read-only support. The first workflow
attempt refused before agent dispatch because max-only qualification controls
were paired with xhigh; it produced no verdict and is inadmissible. After those
controls were removed and the config revalidated, the one counted read-only
pass ran as execution `exec-5319a4fc9a69c08bfe8930b939f195d5`, attempt
`attempt-47e8e46d4cc15a412aaac75b1628eeda`.

The node completed with primary `completed`, cleanup `complete`, no cause, and
no provider acceptance. Prism/LSP were unavailable to the reviewer; it used
bounded repository-local Git/search fallback. It ran no builds or tests.

Terminal artifact: 16,340 bytes / 118 lines; SHA-256
`6a690d6191f1d2faadcb208b1ddcf03c19525bb1d75d6e188cab554b555ad6e2`.

## WRONG findings returned by the lens

1. **WRONG / BLOCKER — recovery can terminalize or erase a live request.**
   Every `bind_remote_request` runs attempt-wide reservation recovery. The
   census does not exclude the registry's live flights or serialize the whole
   recovery-plus-admission boundary. A concurrent successor can roll back a
   predecessor's zero-row reservation or terminal-CAS its journaled live intent
   to `Unknown`. Required repair: positive quiescence/death proof plus one
   serialized admission boundary and live-registry exclusion. Red regression:
   barrier distinct A/B requests at zero-row and journaled-intent cuts and prove
   A stays live and settles `Complete`.

2. **WRONG / BLOCKER — missing settlement observation collapses to cleanup
   `Complete`.** Active LegacyV2 slots intentionally have no V3 settlement
   handle; V3 also has a durable-bind-to-slot-publication window, and terminal
   refusal can clear the only active observer. `cleanup_session_checked` maps
   all of these `None` states to `Complete`. Required repair: distinguish no
   request, Legacy active, admission pending, and retained result/refusal;
   project `Complete` only from positive proof. Red regressions cover active
   Legacy, admission barrier, and unresolved terminal-refusal states.

3. **WRONG / BLOCKER — drop-time settlement refusal is erased.**
   `RequestScope::drop` and the durable wrapper ignore settlement errors and
   clear the exact active slot, leaving no diagnostic/cleanup owner. Required
   repair: transfer the dropped driver and acceptance-aware diagnostic context
   to owned cleanup state. Red regression: accepted stream drop plus terminal
   journal refusal must yield persistence/fatal, accepted=true, cleanup
   `Unknown`.

4. **WRONG / BLOCKER — cleanup timeout detaches an unbounded blocking
   waiter.** Tokio timeout drops a `spawn_blocking` join handle but cannot stop
   `join_blocking`, whose condition-variable wait has no deadline. A retained
   unpolled stream can strand one blocking worker per cleanup. Required repair:
   deadline-aware core wait or async observation whose worker itself exits. Red
   regression proves both `Unknown` and observer termination.

5. **WRONG / BLOCKER — nonzero pre-intent prefixes are never recovered.**
   Recovery acts only after `IntentJournaled`; a crash after `FlightReserved` or
   `RemoteRequestIdentityCaptured` leaves a permanent nonterminal reservation.
   Required repair: enumerate and terminalize every valid nonempty request
   prefix protectively, with owner evidence where available. Red regressions
   cover every pre-intent crash cut and repeated reopen.

6. **WRONG / BLOCKER — the bounded census is not an admission bound.**
   Discovery refuses above 4,096 retained reservations, but reservation
   creation never enforces that population limit and terminal reservation files
   are never retired. The 4,097th request is admitted; every later census then
   returns `Full`, permanently refusing successors. Required repair: atomic
   population admission and bounded terminal compaction/tombstones. Red
   regression uses a small injected limit and proves over-cap refuses before
   reservation creation.

7. **WRONG / BLOCKER — durable terminal CAS and publication are not one
   recoverable operation.** Recovery fsyncs `Settled` and then invokes a void
   publisher. Process death between them yields zero publications; reopen sees
   `AlreadySettled` and never publishes. Required repair: durable outbox/delivery
   marker and idempotent consumer identity, or an equivalent acknowledgeable
   protocol. Red fault injection stops after terminal CAS, reopens, and observes
   exactly one aggregation.

8. **WRONG / BLOCKER — journal-root authority remains path-based.** `open`
   checks metadata and then calls a helper that can recreate a removed root;
   after open, rename-plus-replacement can redirect lock and data operations to
   a different directory at the same path. Required repair: existing-only open,
   retained root identity, and descriptor-relative lock/data operations. Red
   barriers cover removal between metadata/lock and replacement after open.

## SMELL returned by the lens

The recovery tests are not behaviorally fail-first for attempt-wide discovery:
the process regression reuses the same key, which the parent already recovered
through the registry's same-key path. Required live/concurrent, pre-intent,
Legacy cleanup, timeout-worker, population-bound, crash-publication, and
root-replacement edges remain absent. The lens classified this as DEFER because
the corresponding runtime defects already block.

## Lens verdict

Requirements 1, 2, and 7 passed. Requirements 3-6 failed for the mechanisms
above. The exact terminal form was:

`VERDICT: REJECT`

`SUMMARY: Eight BLOCKER WRONGs remain in checked-cleanup truthfulness, drop/timeout handling, live and crash recovery, bounded reservations, publication replay, and file-journal root identity.`
