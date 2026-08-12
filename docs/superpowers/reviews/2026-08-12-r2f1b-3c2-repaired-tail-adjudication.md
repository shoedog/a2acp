# R2f1b 3c2 repaired-tail adjudication — OPEN-CLASS PARK

Date: 2026-08-12.
Reviewed feature artifact: `cecff376e4e3c5b705d83cf21f402203ae2a9583`.
Reconciled feature handoff head: `530992b7`.
Original main/base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`.

## Round cap and classification

The repaired-tail cap was one Sol/xhigh read-only review pass. It returned
`REJECT` with eight BLOCKER WRONGs after the preceding dual round's six confirmed
WRONGs and the +800-net-line targeted repair. No additional repair or review
round is authorized silently.

Operator source adjudication confirms that the new population is larger, spans
new instances of the same custody/authority class, and requires subsystem
design rather than a bounded tail patch: attempt admission serialization and
deadness proof, cleanup-result retention, owned drop cleanup, deadline-aware
observation, complete crash-prefix recovery, bounded reservation retirement,
durable publication outbox/ack, and descriptor-root custody. Classification:
**open-class at cap; park and escalate to spec/design**.

## Operator finding rulings

### T1 — live request terminalized/erased by successor recovery — CONFIRMED WRONG, BLOCKER

`bind_remote_request` calls `recover_remote_request_reservations` before every
new reservation. The census operates on all durable request reservations and
never checks `ResourceFlightRegistryV1::flights`. Its per-operation file lock
does not span recovery plus the predecessor's whole reservation/create/intent
sequence. Therefore distinct concurrent request B can remove A's zero-row
reservation or write A's durable `Unknown` terminal while A is live. This
directly contradicts the repair's “abandoned/dead” recovery premise.

### T2 — cleanup `None` collapses unresolved states — CONFIRMED WRONG, BLOCKER, with one refinement

Active LegacyV2 has an active slot but no V3 settlement handle and therefore
returns `Complete` immediately after signaling. Concurrent V3 cleanup can also
remove a session between durable bind and active-slot publication, returning
before the flight settles. A terminal-refusal/drop path clears the only slot and
leaves later checked cleanup unable to observe unresolved debt. Those are
constructible wrong outputs.

The review's broader statement that every previously settled `Partial`,
`Failed`, or `Unknown` result must permanently taint a later no-active-request
cleanup is not established: a durable already-published request result need not
be the outcome of a later independent session cleanup. This refinement does not
collapse the live Legacy, admission-window, or unresolved-refusal blockers.

### T3 — drop settlement refusal erased — CONFIRMED WRONG, BLOCKER

Both drop implementations ignore the settlement result. `RequestScope::drop`
then clears the exact active slot. No synchronous or asynchronous diagnostic
owner remains to preserve persistence/fatal plus accepted=true. R3 explicitly
required drop/cleanup settlement failures to remain acceptance-aware.

### T4 — timeout leaves blocking waiter alive — CONFIRMED WRONG, BLOCKER

`tokio::time::timeout` cancels only the async wait on the `JoinHandle`.
`spawn_blocking` continues running and `RetainedResourceFlight::join_blocking`
has no deadline. A stream owner that retains an already-published scope without
polling cancellation can keep the condition variable nonterminal indefinitely.
The caller's `Unknown` is protective, but the cleanup mechanism is not bounded.

### T5 — nonzero pre-intent crash prefixes strand — CONFIRMED WRONG, BLOCKER

`recover_journaled_intent_as_unknown` returns `None` unless an
`IntentJournaled` row exists. The census ignores that `None`. `FlightReserved`
and `RemoteRequestIdentityCaptured` crash cuts are neither exact-zero rollback
nor journaled-intent recovery and therefore remain durable nonterminal debt.

### T6 — reservation population can cross its own census cap — CONFIRMED WRONG, BLOCKER

Both journal implementations limit only the returned census. Neither
`reserve_flight` enforces `MAX_DISCOVERED_RESOURCE_FLIGHT_RESERVATIONS`, and
terminal settlement never removes reservation authority. At 4,096 entries one
more request is created; all later recovery calls fail `Full`. This is a normal
long-lived-attempt outage once V3 is armed, not a theoretical malformed input.
The prior dual adjudication already ledgered unbounded per-request growth for
pre-arming retirement; the repair made that ledger item an actual recovery
admission brick.

### T7 — crash gap between terminal CAS and publication — CONFIRMED WRONG, BLOCKER

The repair mandate requires one publication on reopen and idempotent no-second
publication. Terminal append is durable before the void publisher call. Death
in that interval yields zero, while reopen sees `AlreadySettled` and suppresses
publication. Closing this requires an acknowledged durable publication protocol
or an explicit weakening of the specification; it is not a local retry.

### T8 — journal root can be recreated or substituted — CONFIRMED WRONG, BLOCKER

The new `open` sequence runs metadata first, then the create-capable persistent
lock helper. Removing the root between those steps recreates it. After open,
only paths are retained; replacing the directory and lock at the same spelling
redirects existing handles. The repair's existing-only operation lock closes
the simple removed-root test but does not bind the directory object across open
or later operations. This is the immutable-custody class already enforced at
other filesystem destruction/publication boundaries.

## Stable evidence

- Initial reviewed artifact: `772518a8`.
- Repair implementation commit: `cecff376`.
- Green acceptance gates before repaired-tail review: diff/format/clippy,
  release build, dependency policy, hygiene, and full host workspace
  **3,980 passed / 0 failed / 13 ignored across 90 harnesses**.
- Repaired-tail Sol/xhigh lens record:
  `2026-08-12-r2f1b-3c2-repaired-tail-solxhigh.md`.
- Terminal review artifact: 16,340 bytes, SHA-256
  `6a690d6191f1d2faadcb208b1ddcf03c19525bb1d75d6e188cab554b555ad6e2`.
- Feature handoff reconciled and committed at `530992b7`.

Green deterministic gates prove internal consistency, not closure of the
untested concurrency/crash mechanisms. Production API V3 remains explicitly
unarmed, which limits current exposure for T1/T3/T5-T8 but does not make the
authority implementation acceptable for its planned activation. T2 reaches
current LegacyV2 checked cleanup.

## Binding next state

Do not fold or push `feat/r2f1b-3c2-api-authority`; do not advance to 3d as if
3c2 landed. Before another implementation round, revise the 3c2 design into
smaller independently reviewable tasks with explicit ownership for:

1. quiescent attempt-start recovery versus live request admission;
2. exact cleanup state/result retention plus deadline-aware observation;
3. drop-owned settlement/diagnostic custody;
4. complete prefix recovery and bounded reservation retirement;
5. durable publication acknowledgement/outbox semantics; and
6. descriptor-relative journal-root identity.

The preserved feature branch/worktree is evidence and a partial artifact, not
an approved landing candidate. Restarting from a fresh implementation requires
explicit owner authorization and a written unsalvageability reason.
