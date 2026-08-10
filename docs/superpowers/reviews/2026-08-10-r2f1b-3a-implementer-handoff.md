# R2f1b 3a implementer handoff — retained resource flight core

Date: 2026-08-10
Scope: feat/r2f1b-3a-flight-core sub-slice 3a, flight core only.

## Delivered surface

| Obligation | Implementation / evidence |
|---|---|
| One transition-locked flight | RetainedResourceFlight uses one Mutex<State> plus Condvar; attach, close, intent, discovery, dispatch admission, guard transfer, and settlement serialize through it. |
| Durable journal before dispatch | FileResourceFlightJournal writes versioned JSONL records, syncs the record and root, and only begin_journaled_dispatch can enter Signaling. It refuses from AdmissionClosed. |
| Bounded accounting | Every flight record is contiguous and sequenced. IntentJournaled reserves dispatch_started, guard_transferred, and settled lifecycle capacity; ordinary owners/discoveries cannot use those slots. |
| Owners and delivery | Owner data has a node and canonical owner key. Close snapshots a BTreeSet deterministically; attachment then refuses. Discovery is separately journaled post-close. A winning durable terminal append publishes exactly one result per snapshotted or collateral-discovered owner. Publication follows durable `Settled`, so the disclosed crash window is at-most-once rather than exactly-once across a process crash; Slice 5 owns the durable outbox. |
| Cardinality | ResourceFlightRegistryV1 is bound to one AttemptId and holds a strong Arc for each live key. Its durable reservation CAS writes one create-new, synced key-to-flight record before FlightReserved; one registry per attempt is mandatory for 3b1. A later registry returns the original terminal result (or terminally recovers a journaled intent as Unknown) rather than minting another generation flight. RetainedResourceFlight::create is module-private, making the registry reservation CAS the only public acquisition path. Distinct remote-request keys mint independent flights. |
| Dead recovery | Recovery takes only a journal, flight id, duration, and result publisher. It has no identity, PID, process, container, or dispatch capability; a journaled unsettled flight terminal-CASes to Unknown, then only the winning append publishes one aggregation to the durable admission snapshot plus every collateral-discovered owner. A later recovery sees or loses to Settled and publishes nothing. Positively-proven-dead liveness remains Slice 5. |
| Owned process tree | OwnedProcessTreeV1 captures a SHA-256 spawn nonce and supplied ProcessStartIdentityV1, then journals it. It has no signal, process, backend, or platform-probe method. |
| Row 10 mechanism | retain returns a non-cloneable guard. At a supplied MonotonicClock deadline, transfer_cleanup_deadline journals GuardTransferred before Partial settlement with RecoveryOwnerV1 and returns the exact guard. A foreign or missing guard, or a flight that adopts an already-durable recovery terminal, returns Unknown without asserting a new transfer. No timer is armed; the prepare/accept guard-transfer redesign is a Slice-5 ledger item. |
| Blocking join | join_blocking is a std::sync::Condvar wait and never enters Tokio. |
| Slice-5 boundary | Settlement publishes one NodeCleanupAggregationV1 through ResourceFlightResultPublisher for every owner. Slice 5 joins those values and remains the production writer of NodeCleanupRecordV2.collateral. |

## Design notes

1. Journal substrate, capacity, and sequence: ResourceFlightJournal is the durable storage port. Its file implementation is append-only JSONL below an already-owned attempt root. Schema is v1 and sequence starts at one, validated on read and append. Intent admission atomically reserves lifecycle accounting before an inert dispatch admission exists. The runner refuses a missing/non-directory root rather than inventing an unpinned durable parent.

2. Cardinality and aggregation: one ACP/container generation maps to one ResourceFlightKeyV1 and live Arc; each dedicated remote request uses its request id key. The registry is AttemptId-bound, retains every live Arc, and first wins a durable create-new, fsynced reservation record keyed by the SHA-256 of the canonical key. 3b1 must retain one registry per attempt. Existing bindings cannot be overwritten: live callers join the Arc; another registry converges through the journal terminal CAS rather than minting or overriding a flight. FlightReserved and OwnerAttached record the flight-local and node-owner sides. Settlement publishes the actual NodeCleanupAggregationV1 value once per owner through ResourceFlightResultPublisher, including the original flight id, result, shared collateral disposition, and affected_owner_count. Slice 5 joins these values to form and persist NodeCleanupRecordV2.collateral.

3. Sync join: join_blocking waits on the same Condvar terminal state used for publication. It is non-async by construction and meets the bare-thread terminate_blocking constraint.

## Required regressions added

- admission_closed_without_journaled_intent_permits_no_signal
- bounded journal capacity refusal before dispatch
- deterministic snapshot order, collateral discovery delivery, and attach-after-close refusal
- both serialized attach-vs-close orders
- second requester joins the same generation flight; the registry retains it after caller handles drop; dedicated request cardinality
- durable generation reservation survives registry loss: a dead journaled flight settles Unknown and the requested replacement id has no journal
- a_dead_journaled_flight_settles_unknown_and_publishes_to_every_owner: snapshot owners plus collateral receive exactly one Unknown; a second recovery publishes nothing
- a_generation_flight_is_unobtainable_outside_the_registry_reservation plus an external compile_fail proof: a second reservation joins the same Arc and no downstream constructor is visible
- dead journaled flight settles Unknown without identity reconstruction
- crash/reopen reads a durable journaled intent
- cleanup_deadline_transfers_exact_guard_before_terminal
- cleanup_deadline_adopts_recovered_terminal_without_transfer
- failed exact-guard transfer settles Unknown
- non-async join_blocking
- process-tree identity capture journal hook
- exact-capacity lifecycle-reservation completion
- live_flight_adopts_recovery_terminal_and_publishes_once (recovery after dispatch)
- live_flight_converges_when_recovery_settles_before_dispatch (recovery before dispatch)
- recovery_terminal_cas_uses_sequence_after_a_stale_snapshot (a concurrent collateral row cannot stale the terminal sequence or omit its owner)

The discriminating mutation checks are represented by those regressions: allowing dispatch from AdmissionClosed re-reds row 7; dropping/accepting a foreign transferred guard re-reds row 10; the two serial attachment/close orders cover their transition mutation; skipping recovered owner publication re-reds a_dead_journaled_flight_settles_unknown_and_publishes_to_every_owner; and re-exposing pub create makes the compile_fail constructor proof re-red. Runtime observations remain blocked by the environment below.

## Lock declaration and custody review

Read before adding synchronization: crates/bridge-worktree/src/custody_lock.rs header, including its deletion_admission prohibition. The flight transition and registry mutexes are both declared in the module header. It never takes a custody file cell, run/operation lock, or deletion_admission; journal work is inside its owning mutex while publication is outside both. No callback may re-enter either the flight or registry mutex.

## §2c self-pass

Predicate: no signal-shaped effect is reachable through the runner without a durably journaled, admission-closed intent; every snapshotted or pre-settlement discovered owner receives exactly one result, including recovery; a flight is unobtainable outside the reservation CAS; the runner is joinable from non-async contexts; and nothing in 3a can deliver a real signal.

Search scope: crates/bridge-core/src/retained_resource_flight.rs and its resource_flight.rs re-export. Search used:

    rg -n 'kill|signal|terminate|libc|tokio|AgentBackend|deletion_admission|custody' crates/bridge-core/src/retained_resource_flight.rs
    rg -n 'pub fn create' crates/bridge-core/src/retained_resource_flight.rs
    rg -n 'RetainedResourceFlight::create' crates/bridge-core/src/retained_resource_flight.rs

Matches are explanatory comments only; no process/container/backend/custody call exists. The only dispatch operation returns an inert marker without an effectful method.

Verdict: **SURVIVED in source.** Journal transition is the only route to Signaling; post-close attachment refuses; recovery derives recipients from durable owner records and publishes only after the Unknown settlement row; discovery/settle contend on one lock; exact transfer is journaled before terminal publication; and this module has no signal-capable dependency. The public-constructor scan is empty; the only create call is the registry Created branch, while the external compile_fail proof rejects downstream acquisition. Compile-dependent evidence remains pending the registry failure below.

## Post-review repair

The verifier reached bridge-core Clippy and rejected the original
RetainedResourceFlightError because TransitionRefused embedded the large
ResourceFlightStateV1 value. The repair boxes that payload and updates the
row-7 assertion to inspect the boxed state. This preserves the diagnostic
state while making every Result error arm small enough for
clippy::result_large_err.

The newer verifier also showed that the test gate was **red**, not blocked:
it reached `running 1073 tests`, then the bridge-core unit-test binary ended
with exit 101. Its retained excerpt includes a `bridge_core::liveness`
advisory-lock release error (`Bad file descriptor`), a known hermetic
flock/exec family, but the truncation omits the failing test name and does not
establish that it is an allowed exclusion. The prior “no tests ran” statement
is therefore retracted. This repair does not claim a green rerun: local Cargo
still reaches the dependency-registry CONNECT 403 before compiling.

The same review exposed two runner defects. A weak registry map let a dropped
flight be re-minted, and the declared aggregation contract never crossed a
port. The registry now retains live arcs, persists a create-new reservation
before local FlightReserved, and terminally recovers a dead journaled binding
as Unknown. ResourceFlightResultPublisher now receives the actual
NodeCleanupAggregationV1 publication per owner. The new regressions pin both
properties.

## Continuation closure — D1–D3

D1–D3 are closed in source, with runtime red/green observation blocked before compilation by the current-host dependency registry failure below.

| Defect | Closure / evidence |
|---|---|
| D1 | FileResourceFlightJournal::read_reservation now explicitly deserializes ResourceFlightReservationRecordV1, removing E0282 at retained_resource_flight.rs:417. |
| D2 | Recovery collects the journaled owner_snapshot and every CollateralDiscovered owner into a deterministic set, appends Settled Unknown, then uses the same publisher path as active settlement. The focused regression asserts three Unknown publications and no fourth publication on a second recovery. |
| D3 | RetainedResourceFlight::create is module-private. Every test helper now obtains its flight through ResourceFlightRegistryV1::reserve, and the external compile_fail proof fails if pub create is restored. The named regression verifies second reservation returns Joined over the same Arc. |

Red-first status: both focused tests were added while the corresponding defects remained. An external verifier later reached bridge-core and exposed a test-only E0382: the D2 regression moved primary into intent before reusing it in its expected-owner set. This continuation passes primary.clone() to intent, retaining the expected owner. The current-host focused rerun could not reach rustc because a2a-lf resolution failed through the host CONNECT tunnel, so no current-host runtime green result is claimed. Mutation checks are source-discriminated: omitting recovery publication leaves the D2 assertion at zero publications, and restoring pub create makes the D3 compile_fail proof compile unexpectedly.

## Repair round 3a — RA–RF

| Item | Closure / regression evidence |
|---|---|
| RA terminal CAS | `ResourceFlightJournal::append_terminal` now assigns the terminal sequence itself under the append lock and returns the exact durable rows used for that decision. Recovery derives recipients from those returned rows, so a concurrent collateral append cannot stale the terminal sequence or omit its owner. `adopt_durable_terminal_locked` reconciles an existing terminal before dispatch and after a raced non-terminal append; a live flight then exposes that terminal to joiners and does not republish. Regressions: `live_flight_adopts_recovery_terminal_and_publishes_once`, `live_flight_converges_when_recovery_settles_before_dispatch`, `recovery_terminal_cas_uses_sequence_after_a_stale_snapshot`, and `concurrent_recovery_has_one_terminal_cas_winner`. |
| RB join refusal | A failed terminal append stores an in-memory `TerminalRefusal`, wakes the Condvar, and returns the typed journal cause to bare-thread joiners. Regression: `terminal_append_failure_refuses_bare_thread_joiner`; removing the failure-path notification leaves the watchdog without its response. |
| RC crash windows | Reservation publication now writes and fsyncs a temporary regular file before no-replace link publication; an empty legacy/torn reservation is retried. File reads truncate exactly one unterminated trailing JSONL record after the earlier records are readable. Regressions: `zero_length_reservation_is_replaced_by_a_complete_durable_record`, `torn_journal_tail_is_truncated_and_recovery_settles_unknown`. Removing tail tolerance makes the latter red. Cross-handle/process serialization of the journal is a Slice-5 ledger row. |
| RD registry callback rule | Durable recovery completes while the registry mutex is held, then deferred publication occurs after releasing it. Regression: `recovery_publication_runs_after_releasing_registry_mutex` re-enters `reserve` under a bounded watchdog. |
| RE | Aggregations carry `affected_owner_count`; the row-10 sequence order is asserted; the constructor doctest is `compile_fail,E0624`; failed foreign transfer returns its guard; adopted recovery terminals return `Unknown` with their exact guard rather than falsely reporting `Transferred`; registry fast-path attempt mismatch refuses; test helpers are cfg(test), dispatch admission is opaque; and discovery is covered in `Signaling` and `Settled`. |

The named mutation controls are structural discriminants in this repair tree. Runtime mutation observation cannot be claimed from this clone: Cargo resolution is blocked before rustc by the host CONNECT 403; the original controls are not represented as a passing result here.

`records()` exposes durable records, including journaled process identities and PIDs. 3b1 must never derive a signal path from those records; platform probes and the live capability binding remain its separate authority boundary.

## Gates and provenance

The stale continuation narrative is superseded as broad host provenance by the operator-recorded R2f1b host aggregate: **3,150 passed / 0 failed / 12 ignored** (`docs/reliability-execution-roadmap.md`, R2f1b closure paragraph). That historical host-green total is not attributed to this uncompiled repair clone.

| Gate | Result |
|---|---|
| historical host aggregate | PASS: 3,150 passed / 0 failed / 12 ignored (operator provenance above) |
| cargo fmt --all -- --check | PASS (exit 0, current repair clone) |
| git diff --check | PASS (exit 0, current repair clone) |
| CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets -- -D warnings | BLOCKED before compilation on current host: a2a-lf registry download CONNECT 403 |
| CARGO_INCREMENTAL=0 cargo test -p bridge-core -p bridge-worktree -p bridge-coordinator -p bridge-controller -p bridge-workflow -p a2a-bridge | BLOCKED before compilation by the same dependency proxy; no current-clone total claimed |
| focused RA–RD runtime/mutation observations | BLOCKED before rustc by the same proxy; the source regressions and their counterfactuals are recorded above |

No provider, smoke, compatibility, release, or deployment action was attempted.

## Declared remainders

- 3b1 owns platform start-identity probes, flight-before-spawn binding, and every real process signal path.
- 3b2/3c1/3c2 own backend/wrapper forwarding and destructive adapter joining.
- Slice 4 owns production deadline arming.
- Slice 5 owns durable node-cleanup collateral aggregation and recovery persistence, the durable publication outbox, positively-proven-dead liveness, prepare/accept guard transfer, and cross-handle journal serialization.
- Re-run focused/full gates when the registry is reachable; current runtime and mutation evidence remains unclaimed rather than inferred from the historical host aggregate.

## Operator gate addendum (post-repair, host, darwin — supersedes the BLOCKED rows above)

The clone's CONNECT 403 was the 2026-08-09 20:15–21:10 MDT house power/network
outage overlapping this run — NOT an egress-proxy degradation (the run log
contains no 403 events; the count of real proxy degradations stays at 2).
After landing `296e6968` the operator completed the blind tail (the missing
`AtomicBool` test import; one moved-value fix at the CAS-winner test's intent
call) and ran the full block on host:

```
git diff --check / cargo fmt --all -- --check          clean
cargo clippy --workspace --all-targets -- -D warnings  exit 0
six-package suite                                      2705 passed / 0 failed / 11 ignored (51 binaries)
```

The RA–RF mutation controls recorded above as structural discriminants were
therefore exercised as REAL runs at the fold (the new regressions all ran
green; the torn-tail and CAS-winner reds are in the suite). Provenance:
operator, worktree `s3a`, heads `296e6968` + the completion commit.
