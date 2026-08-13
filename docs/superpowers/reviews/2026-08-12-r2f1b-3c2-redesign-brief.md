---
task-type: design
---
# R2f1b 3c2 salvage-first request-flight redesign

## Description

Design the next implementation sequence for the parked R2f1b 3c2 API
request-flight artifact. Work against the exact checkout head
`530992b7ff1e8e9151fb2a69e86f3ff71c44f905`, whose original main base is
`42249b3d926b49afd9d0dbd213d0ee3d3e459af6`.

The artifact is preserved evidence and a partial implementation, not an
approved landing candidate. Start salvage-first: identify which existing
mechanisms can remain, which require bounded revision, and which mechanism
must be replaced. Do not propose a fresh restart unless you provide a written
mechanism-level unsalvageability proof.

The prior repaired-tail review and operator adjudication confirmed eight
BLOCKER WRONGs:

1. Per-bind recovery can terminalize or erase another live request because it
   has neither quiescence/deadness proof nor recovery-plus-admission exclusion.
2. Checked cleanup maps active LegacyV2, the V3 bind-to-slot publication
   window, and lost terminal-refusal/drop observation to false `Complete`.
3. Drop ignores settlement refusal and clears the only observer, losing the
   acceptance-aware persistence diagnostic.
4. Timing out an async wait drops only the `JoinHandle`; the underlying
   `spawn_blocking(join_blocking)` waiter can remain alive indefinitely.
5. Nonzero pre-intent crash prefixes (`FlightReserved` and
   `RemoteRequestIdentityCaptured`) remain nonterminal and unrecovered.
6. Admission can exceed the 4,096-entry census cap; terminal reservations are
   not retired, so the 4,097th request can brick all later recovery/admission.
7. Terminal CAS is durable before the one void result publication; a crash in
   that gap yields zero publication and reopen suppresses it as already
   settled.
8. The file journal retains path authority rather than an immutable directory
   object; open and operation races can recreate or redirect the root.

The source of truth for those rulings is
`docs/superpowers/reviews/2026-08-12-r2f1b-3c2-repaired-tail-adjudication.md`.
The adjacent repaired-tail lens and the feature handoff are maps and claims,
not proof. Inspect the code and production callers directly.

## Acceptance Criteria

- Produce a coherent authority/state-machine design that closes all eight
  confirmed WRONGs without weakening prompt-acceptance, retry, cleanup, or
  custody semantics.
- Give a component-level salvage ruling: KEEP, REVISE, or REPLACE, with exact
  source seams and a mechanism reason. Preserve request identity, global
  forget/recreate ABA protection, structured acceptance-aware diagnostics, and
  the production-unarmed route where they remain sound.
- Separate quiescent attempt-start recovery from live request admission. State
  the exact owner, lock/lease ordering, live-request exclusion proof, crash
  prefixes, and restart behavior.
- Define exact checked-cleanup observation for LegacyV2 and ProtectedV3,
  including admission-before-slot, active, terminal, refusal, timeout, and
  drop states. Only a proven durable `Complete` may project `Complete`.
- Give drop-owned settlement/diagnostic custody with a bounded lifetime. No
  detached blocking waiter may survive the caller's declared cleanup bound.
- Define a bounded durable lifecycle: reservation admission, full prefix
  recovery, terminal retirement/compaction, capacity accounting, and behavior
  at and beyond the 4,096 boundary.
- Define durable exactly-once publication semantics across every crash cut.
  Prefer an explicit outbox/ack protocol; if the contract must be weakened,
  identify the exact observable promise removed and why that is acceptable.
- Define descriptor-relative journal-root identity and operation rules on the
  supported Unix hosts, including remove/rename-replace races and the behavior
  when immutable root custody cannot be established.
- Preserve the binding carry-forward: the two-field inner/outer cleanup split
  is mandatory in whichever later slice first arms production V3 or wraps
  `ContainerRw`; this design must not accidentally trigger or erase it.
- Production remains unarmed (`resource_flight_route_v3 = None`), no provider,
  smoke, compatibility, deployment, or running-operator action is proposed,
  and 3d remains blocked until 3c2 lands.
- Split the work into independently reviewable tasks that each compile and
  leave the whole repository green. For every task specify: frozen input
  commit, owned files/seams, exact APIs/types/state transitions, red tests that
  fail on the current artifact, implementation ripple/call sites/test doubles,
  focused gates, full-suite boundary, commit boundary, dependencies, and an
  explicit stop/split threshold before the task becomes a multi-thousand-line
  big bang.
- Order the tasks so infrastructure contracts land before consumers and no
  intermediate commit makes the dormant V3 path less safe or changes current
  production behavior accidentally.
- Identify decision points requiring owner judgment, any residual SMELL/DEFER
  items, and the evidence that would make the preserved artifact genuinely
  unsalvageable.

## Spec Refs

- `docs/superpowers/reviews/2026-08-12-r2f1b-3c2-repaired-tail-adjudication.md`
- `docs/superpowers/reviews/2026-08-12-r2f1b-3c2-repaired-tail-solxhigh.md`
- `docs/superpowers/reviews/2026-08-12-r2f1b-3c2-dual-adjudication.md`
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
- `docs/superpowers/plans/2026-08-09-r2f1b-slice3-brief.md`
- `docs/reliability-execution-roadmap.md`

## Constraints

This is a hard read-only design turn. Do not edit files, build, test, install,
invoke another provider, use network access, or start nested helpers. Complete
one pass and return one self-contained design; do not implement it.
