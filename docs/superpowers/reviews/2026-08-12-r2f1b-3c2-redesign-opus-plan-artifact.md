# R2f1b 3c2 — salvage-first request-flight authority redesign

Frozen input: `530992b7ff1e8e9151fb2a69e86f3ff71c44f905` (branch
`feat/r2f1b-3c2-api-authority`). Original main base:
`42249b3d926b49afd9d0dbd213d0ee3d3e459af6`.

## Context

The 3c2 artifact gives every remote HTTP round its own journaled
`DedicatedRemoteRequest` flight and scopes cancellation by `(turn_epoch,
request_identity)`. Deterministic gates are green (3,980/0/13 across 90
harnesses), but a Sol/xhigh repaired-tail review plus operator source
adjudication confirmed eight BLOCKER WRONGs in the *untested* concurrency,
crash-recovery, cleanup-observation, bounded-storage, durable-publication and
filesystem-custody mechanisms. 3c2 was parked as open-class at cap.

Root cause is not the mechanisms individually — it is that one ~2,700-line
implementation commit landed six independent authority contracts at once
(brief estimate was ~1,500 with a 2,250 stop threshold). Every confirmed WRONG
lies in a *different* authority. This design separates them into seven
independently reviewable tasks along seams that already exist in the source.

**Salvage verdict: SALVAGEABLE.** Nothing requires a schema change, no
already-landed 3a/3b1/3b2/3c1 consumer contract has to move, and every fix
decomposes along an existing seam. Full unsalvageability criteria in §6.

---

## 1. Invariants and state machines

### 1.1 Authority separation (the thing the artifact does not have)

Four authorities are conflated today. The design names one owner each.

| Authority | Owner | Scope | Excluded from |
| --- | --- | --- | --- |
| A1 Attempt recovery | `DurableProcessFlightAttemptV3` | once, at attempt start, under attempt lease | any live request |
| A2 Request admission | `ResourceFlightRegistryV1::reserve` | per request, under `flights` | recovery |
| A3 Request lifecycle | `RetainedResourceFlight` | per flight, under `transition` | other flights |
| A4 Session cleanup observation | `ApiBackend` session map | per session | flight mutation |

Today A1 runs *inside* A2 (`bind_remote_request` →
`recover_remote_request_reservations`, `process.rs:954`) with no lock spanning
both and no consultation of `flights`. That single inversion is T1.

### 1.2 Global lock order (declared, total)

```
attempt lease (flock, held for attempt-handle lifetime)
  → ResourceFlightRegistryV1::flights          (std Mutex)
    → journal operation lock (append_lock Mutex → flock on retained lock fd)
      → RetainedResourceFlight::transition     (std Mutex)
```

No callback runs under any of these. Result publication and every
`ResourceFlightResultPublisher::publish` call happen strictly outside all four.
This matches the order the artifact already takes in
`ResourceFlightRegistryV1::reserve` (`retained_resource_flight.rs:2169`); the
design only makes it explicit and makes recovery obey it.

### 1.3 Core invariants

- **I1 (deadness proof).** Recovery may terminalize or retire a reservation
  only when (a) this process holds the attempt lease exclusively, and (b)
  `flights` is held and empty for the whole census. Both conditions are
  mechanical, not conventional.
- **I2 (admission exclusion).** No reservation is created, rolled back, or
  terminalized by anyone but the `flights` holder.
- **I3 (total prefix function).** Every durable request-reservation row prefix
  maps to exactly one recovery class. There is no "no matching rule → leave it".
- **I4 (proof-only Complete).** A cleanup disposition of `Complete` is emitted
  only from a proven durable `Complete` terminal or from a proven absence of
  any request debt. Every unprovable state projects `Unknown`.
- **I5 (bounded observation).** No waiter created by a cleanup call may outlive
  the deadline that cleanup declared. Dropping a handle is not a bound.
- **I6 (publication liveness).** A durable terminal is followed by at least one
  publication, and the durable evidence needed to re-publish survives until an
  acknowledgement is durable.
- **I7 (bounded population).** Live reservations are admission-bounded; terminal
  reservations are retired. Reservation population is O(concurrent in-flight
  requests), not O(lifetime requests).
- **I8 (immutable root).** After open, the journal addresses one directory
  *object*. Replacing or removing the directory at that spelling can only make
  operations refuse, never redirect.
- **I9 (unchanged non-request authority).** ACP/container generation flights,
  schema v1 wire, `LIFECYCLE_SLOTS`/`PROCESS_LIFECYCLE_SLOTS`, and the
  `outstanding()` accounting are untouched. Every new behavior is discriminated
  on `ResourceFlightKeyV1::DedicatedRemoteRequest`.
- **I10 (unarmed production).** `resource_flight_route_v3 = None` in
  `bin/a2a-bridge/src/main.rs:1622` throughout; no intermediate commit arms V3
  or wraps `ContainerRw`.

### 1.4 Request flight state machine (durable)

Journal rows, in order, per request key. Slot accounting is unchanged
(`FlightReserved` reserves `LIFECYCLE_SLOTS = 4`; the four following rows each
consume one).

```
∅ ──reserve_flight──▶ RESERVED₀      (reservation file, zero rows)
RESERVED₀ ──FlightReserved──▶ RESERVED
RESERVED ──RemoteRequestIdentityCaptured{identity,owner}──▶ IDENTIFIED
IDENTIFIED ──IntentJournaled{owner_snapshot}──▶ INTENT
INTENT ──DispatchStarted──▶ DISPATCHED
{RESERVED,IDENTIFIED,INTENT,DISPATCHED} ──Settled{result}──▶ SETTLED
SETTLED ──publish──▶ SETTLED+PUBLISHED   (marker child `<id>.published`)
SETTLED+PUBLISHED ──retire──▶ ∅
```

**Crash-cut recovery table (I3, total).** `⊥` = no such cut.

| Durable cut | Provider effect possible? | Recovery action | Recipients |
| --- | --- | --- | --- |
| `RESERVED₀` (zero rows) | no | rollback reservation | — |
| `RESERVED` | no | terminal-CAS `Failed`, publish, retire | none (no owner known) |
| `IDENTIFIED` | no | terminal-CAS `Failed`, publish, retire | owner from `RemoteRequestIdentityCaptured` |
| `INTENT` | no (`DispatchStarted` is journal-before-POST) | terminal-CAS `Failed`, publish, retire | `IntentJournaled.owner_snapshot` |
| `DISPATCHED` | **yes** | terminal-CAS `Unknown`, publish, retire | `owner_snapshot ∪ CollateralDiscovered` |
| `SETTLED`, no marker | — | publish existing result, mark, retire | derived from records |
| `SETTLED` + marker | — | retire only | — |
| anything else (missing `FlightReserved`, foreign schema, holes) | — | **refuse** `Accounting`; never guess | — |

The `INTENT → Failed` (rather than `Unknown`) ruling is owner decision **D2**;
it is provable because `begin_journaled_dispatch`
(`retained_resource_flight.rs:1469`) fsyncs `DispatchStarted` before
`RequestScope::begin_dispatch` returns and therefore before the POST future is
built (`backend.rs:862-877`).

### 1.5 Session cleanup observation machine (A4)

Replace the current `Option<RemoteRequestSettlementV1>` with a total
observation computed once under the session lock:

```rust
enum SessionRequestObservationV1 {
    NoRequest,                                  // no slot, no ticket, no debt
    LegacyActive,                               // V2 slot; no durable proof exists
    AdmissionInFlight,                          // V3 ticket outstanding, slot unpublished
    Settleable(RemoteRequestSettlementV1),      // V3 slot with a durable observer
    RetainedDebt(RequestDebtV1),                // unproven terminal retained by drop/refusal
}
```

Projection (I4):

| Observation | Disposition | Why |
| --- | --- | --- |
| `NoRequest` | `Complete` | every prior request settled durably and left no debt |
| `LegacyActive` | `Unknown` (**D1**) | no durable custody exists; provider request may still run |
| `AdmissionInFlight` | `Unknown` | the losing admission path will settle `Failed`; not `Complete` either way |
| `Settleable` → durable `Complete` | `Complete` | proven |
| `Settleable` → any other / refusal / deadline | `Unknown` | unproven |
| `RetainedDebt` | `Unknown` | carries `accepted` for the observed diagnostic |

**Deliberate non-invariant (the adjudication's refinement).** A *proven*
durable `Partial`/`Failed`/`Unknown` from a completed round does **not** taint a
later independent session cleanup. Only an *unproven* terminal (settlement
returned `Err`, or a drop that obtained no durable result) creates
`RetainedDebt`. A red test pins this in both directions.

### 1.6 Publication contract (I6) — stated weakening

Exactly-once *delivery* across arbitrary crash cuts is unachievable without a
transactional consumer. The contract becomes:

- **Preserved:** at-least-once delivery; a durable idempotence key
  `(resource_flight_id, owner)` already present on `NodeCleanupAggregationV1`;
  exactly-once *effect* at the consumer.
- **Removed:** "the publisher is called exactly once". The observable window is
  a crash strictly between the publisher call and the durable marker.
- **Binding on slice 5:** its `NodeCleanupRecordV2.collateral` writer must be
  idempotent on `(resource_flight_id, owner)`. Recorded as a cross-slice
  obligation (**D4**).

---

## 2. Salvage map — KEEP / REVISE / REPLACE

| # | Component (exact seam) | Ruling | Mechanism reason |
| --- | --- | --- | --- |
| S1 | `DedicatedRemoteRequestIdV1` (`resource_flight.rs`, +76 lines) | **KEEP** | CSPRNG identity, zero-value/namespace/legacy-shape refusals, schema-v1 `"req-1"` golden byte-identical. No confirmed WRONG touches it. |
| S2 | `ResourceFlightKeyV1::DedicatedRemoteRequest` + `from_identity` | **KEEP** | Typed key is the discriminator every specialization in this design keys on (I9). |
| S3 | `RemoteRequestIdentityCaptured` event + slot accounting (`LIFECYCLE_SLOTS`, `outstanding`) | **KEEP** | 5-row flight fully capacity-reserved before capability creation; goldens and negatives exist; no new rows are added by this design. |
| S4 | Turn-epoch + `(turn_epoch, identity)` cancel/clear fence (`RequestCancelCapability`, `backend.rs:317-362`) | **KEEP** | This is the forget/recreate ABA protection and the stale-round fence. Mutation-sensitive tests exist. Extended, not replaced, in S12. |
| S5 | `TurnScope` epoch-only clearing (`backend.rs:301`) | **KEEP** | Cannot clear a request slot; correct as-is. |
| S6 | Monotonic acceptance barrier + `request_flight_failure` acceptance-aware diagnostics (`backend.rs:136,828-884`) | **KEEP** | R2b structured-diagnostic contract; never cleared between rounds. Reused as the debt's `accepted` source. |
| S7 | Production-unarmed route (`main.rs:1622`, `config.rs`, `resource_flight_v1`) | **KEEP** | I10. Guard test added in Task G. |
| S8 | `settle_request_scope` one-shot `Option<RequestScope>` consumption (`backend.rs:415`) | **KEEP** | Closes the blind-tail duplicate-terminalization defect; borrow-based settle was correctly rejected. |
| S9 | `FileResourceFlightJournal` root/lock authority (`retained_resource_flight.rs:572-632`; `liveness.rs:241`) | **REPLACE** | Path-based authority: `metadata()`-then-create-capable-lock recreates a removed root; post-open ops re-resolve `lock_path`/`root` by path each call, so a rename-replace redirects live handles. No local patch binds the directory *object*. Replace with `PinnedDirectoryV1` + retained lock fd. |
| S10 | `ResourceFlightRegistryV1::recover_remote_request_reservations` (`:2092`) | **REVISE** (relocate + gate) | Census logic is reusable; its *caller* and locking are wrong. Move ownership to the attempt, run once under lease + `flights`-held + empty assertion, delete the per-bind call. |
| S11 | `recover_journaled_intent_as_unknown` (`:1761`) | **REVISE** | Correct for `DISPATCHED`; returns `None` for `RESERVED`/`IDENTIFIED`/`INTENT` and the census discards that `None` (T5). Wrap in a total prefix classifier for request keys only; the shared `Unknown` path for generation keys is unchanged (I9). |
| S12 | `RequestScope::drop` / `settle` slot clearing (`backend.rs:380-413`) | **REVISE** | `clear_exact()` runs unconditionally even when `flight.settle()` returned `Err`, erasing the only observer. Become `clear_exact_with_outcome(Proven|Unproven{accepted})`. |
| S13 | `cleanup_session_checked` (`backend.rs:727-761`) | **REVISE** | `None → Complete` collapses three distinct states; `tokio::time::timeout` bounds only the async wait. Replace the `Option` with `SessionRequestObservationV1`; replace the timeout with a deadline-aware core wait. |
| S14 | `RetainedResourceFlight::join_blocking` (`:1635`) | **KEEP + extend** | Must stay Tokio-free for `Supervised::terminate_blocking`'s bare thread. Add `join_until(deadline_ms)` on `Condvar::wait_timeout`; do not modify the unbounded form. |
| S15 | Terminal CAS → void `publish` (`:1540-1592`, `:1811-1824`) | **REVISE** | Add the durable `.published` marker before ack and re-publish on reopen (I6). |
| S16 | `reserve_flight` population bound + terminal retirement (`:454,737`) | **REVISE** | Bound is enforced only on the *census*, never on creation, and terminal reservations are never retired. Add counter-based admission and `retire_settled_flight`. |
| S17 | `ResourceFlightRegistryV1::flights` map growth | **REVISE** (discovered, not in the eight) | The `BTreeMap` never removes an entry, so one `Arc<RetainedResourceFlight>` per lifetime request leaks in memory for the attempt's life — the in-memory twin of T6. Remove at retirement. |
| S18 | `bind_remote_request` admission-failure path (`process.rs:1019`) | **REVISE** | `let _ = request.settle(Failed)` discards a settlement error on the one path that already has an error to return; fold it into the returned cause. |
| S19 | `DurableRemoteRequestFlightV3::drop` (`process.rs:847`) | **KEEP** (re-scoped) | Once S12 settles before drop, this is reachable only on panic-unwind. Document it as the panic backstop; no sink plumbing needed. |
| S20 | `acquire_existing_persistent_lock_blocking` (`liveness.rs:241`) | **REVISE** | Correct as a narrow fix for the removed-root test, but obsolete once S9 retains the lock fd. Keep the function (it is a reasonable general primitive) but drop the journal's per-operation path re-open. |
| S21 | Wiremock/backend deterministic test corpus (`backend.rs:1758-2674`) | **KEEP** | The stale-round, between-round, collision, capacity, drop, forget, ABA and zero-round tests are the load-bearing coverage and are mutation-sensitive. Extended, never rewritten. |

**Reused rather than rebuilt.** `PinnedDirectoryV1`, `validated_child_name`,
`open_child_no_follow`, `stat_child_no_follow`, `rename_child_no_replace`,
`rename_child_replacing`, `same_open_object`, `pinned_root_unchanged`,
`root_identity_label`, `open_options_create_new_owner_private` and
`FailureCountdownV1` — all already in `crates/bridge-core/src/fs_custody.rs`
and already the sanctioned immutable-custody vocabulary elsewhere in the
workspace. Only three primitives are genuinely missing (Task A).

---

## 3. Alternatives considered and rejected

| Alternative | Rejected because |
| --- | --- |
| Fresh restart of 3c2 | No mechanism is unsalvageable (§6). A restart re-rolls the same distribution and discards mutation-sensitive coverage; convergence discipline forbids it without owner approval + a written unsalvageability reason. |
| Serialize recovery and admission under one big journal lock | Would hold the file lock across an entire POST-preceding admission sequence, coupling unrelated sessions and inverting the declared lock order. Quiescent recovery removes the need for mutual exclusion entirely. |
| Skip live flights during recovery by consulting `flights` only | Closes the same-process race but not the cross-process one, and leaves recovery on the hot path (R3-3). Attempt-lease + once-at-start is strictly stronger and cheaper. |
| Journal rows for publication state (`PublicationPending`/`Published`) | Requires appending after `Settled`, which `append` refuses and `outstanding()` accounting forbids; would move schema v1 goldens. A sibling marker child is invisible to the census and touches no wire. |
| Write the outbox *before* the terminal CAS | The proposed result may lose the CAS to a recovered `AlreadySettled`, leaving a stale outbox that must be rewritten under the same crash exposure. Marker-after-terminal with re-publish-on-reopen has one strictly smaller ambiguous window and an honest at-least-once contract. |
| Claim exactly-once publication with a retry loop | Not achievable across a crash cut. Claiming it is the defect T7 names. |
| Async settlement notification (Tokio `watch`/`Notify`) on the flight | `RetainedResourceFlight` must stay runtime-free for `terminate_blocking`'s bare `std::thread`. `Condvar::wait_timeout` bounds the worker without importing a runtime. |
| Bound the `spawn_blocking` waiter with a longer `tokio::time::timeout` | Any timeout on the *handle* leaves the worker parked; that is precisely T4. |
| Keep terminal reservations forever, raise the cap | Moves the brick, does not remove it; disk and the census scan both grow O(lifetime requests). |
| Move retired journals into a bounded `retired/` ring | Real forensic value, but adds a second population to bound and to recover. Deferred as **D5**; retirement-with-deletion is the default. |
| Re-verify `pinned_root_unchanged` around a path-based `read_dir` census | Weaker than fd-relative enumeration (TOCTOU inside the scan) and the design needs an `fdopendir` helper for the marker sweep anyway. |
| Apply quiescent recovery + retirement to ACP/container generation keys too | Same class, but generation population is per-generation not per-round, so the bound is far away; widening scope re-creates the big-bang defect. Ledgered as **D6**. |
| Return `Retained` instead of `Unknown` for `LegacyActive` | `Retained` asserts a *known retained resource*; the API backend holds none. `Unknown` is the honest lattice value. |

---

## 4. Task sequence — seven tasks, each green, each independently reviewable

Common to every task: branch `feat/r2f1b-3c2-api-authority`; one commit;
**focused gates** = `cargo fmt --all -- --check`, `git diff --check`,
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`,
the task's named tests; **full-suite boundary** = `cargo test --workspace
--locked` before the commit (report totals, do not re-baseline);
**stop/split threshold** = 500 net implementation lines or 900 total
(implementation + tests); at the threshold, split at the named seam and
disclose. Production stays unarmed in every commit.

### Task A — descriptor-relative journal root (closes T8)

- **Frozen input:** `530992b7`.
- **Owned files:** `crates/bridge-core/src/fs_custody.rs`,
  `crates/bridge-core/src/retained_resource_flight.rs`
  (`FileResourceFlightJournal` only), `crates/bridge-core/src/liveness.rs`
  (doc only).
- **APIs/types:** three new `fs_custody` primitives —
  `open_or_create_child_private(parent, name, label) -> File` (`openat`,
  `O_RDWR|O_CREAT|O_NOFOLLOW|O_CLOEXEC`, mode 0600),
  `open_child_append_create(parent, name, label) -> File`,
  `unlink_child_no_follow(parent, name, label)`, and
  `list_children(parent) -> Vec<OsString>` (`fdopendir` on a `dup`ed fd).
  `FileResourceFlightJournal { root: PinnedDirectoryV1, lock: File, cap,
  append_lock: Mutex<()> }`. Every read/append/reserve/rollback/census becomes
  descriptor-relative; `reserve_flight`'s `hard_link` + empty-file-recovery
  branch becomes `rename_child_no_replace`. `operation_lock()` flocks the
  **retained** fd (in-process serialization by `append_lock` first, per the
  declared order). Non-Unix `open()` refuses `Unsupported` rather than
  degrading to path authority.
- **Red tests (fail at `530992b7`):**
  `journal_root_removed_between_probe_and_lock_refuses_instead_of_recreating`;
  `journal_root_replaced_after_open_refuses_every_operation` (rename the root
  aside, create a new directory at the same spelling, assert append/census/
  reserve all refuse and the *new* directory is untouched);
  `journal_child_symlink_is_refused_not_followed`.
- **Ripple:** all `FileResourceFlightJournal::open` call sites are tests
  (`bridge-api:1821,2219`, `bridge-container:2844`, `bridge-acp:12124`,
  `bridge-core` unit tests) — no production caller exists. Windows CI builds
  only `bridge-store` (+ deps), so `bridge-core` must still **compile** on
  non-Unix; no Windows test executes this type.
- **Dependencies:** none. **Why first:** every later task writes through this
  substrate, and it is internal to an unarmed component.

### Task B — bounded durable lifecycle (closes T5, T6; folds S17)

- **Frozen input:** Task A's commit.
- **Owned:** `retained_resource_flight.rs` (journal trait + both
  implementations + registry), `reaper.rs` test decorator.
- **APIs/types:** `ResourceFlightJournal::retire_settled_flight(&self, key,
  id) -> Result<bool>`; journal gains `max_reservations` (default
  `MAX_DISCOVERED_RESOURCE_FLIGHT_RESERVATIONS = 4096`, injectable small for
  tests) and an in-memory population counter seeded by the census, incremented
  on `Created`, decremented on rollback/retire, checked **before** reservation
  creation (`Full` refuses before any capability or POST). New total classifier
  `RequestPrefixClassV1 { Empty, Reserved, Identified, Intent, Dispatched,
  Settled{published}, Invalid }` over durable rows, used only for
  `DedicatedRemoteRequest` keys. Unify the file/in-memory census bound (`>=`
  vs `==` off-by-one). `ResourceFlightRegistryV1::flights` removes the entry at
  retirement (S17).
- **Retirement order (crash-safe, self-healing):** unlink `<id>.jsonl` →
  unlink `<id>.published` → unlink reservation → parent sync. Any crash mid-way
  leaves a zero-row reservation, which the existing `Empty → rollback` path
  already retires; a stray marker is swept by the census.
- **Red tests:** `reservation_population_refuses_before_creation_at_the_bound`
  (injected limit 2, third request refuses with zero new files and no POST);
  `every_pre_intent_crash_prefix_terminalizes_on_reopen` (table-driven over
  `RESERVED`/`IDENTIFIED`/`INTENT`, each asserting disposition, recipients, and
  retirement); `repeated_reopen_is_idempotent_and_publishes_once`;
  `retirement_crash_windows_self_heal_to_rollback`;
  `four_thousand_ninety_seventh_request_does_not_brick_later_admission`.
- **Ripple:** the `reaper.rs` test decorator gains the new trait method;
  generation-key behavior asserted unchanged by existing 3a/3b tests.
- **Dependencies:** A.

### Task C — durable publication marker and journal-derived recipients (closes T7)

- **Frozen input:** Task B's commit.
- **Owned:** `retained_resource_flight.rs` (`settle_locked`, `publish`,
  `recover_journaled_intent_as_unknown`, journal marker primitives).
- **APIs/types:** `ResourceFlightJournal::{publication_marker_present,
  mark_published, sweep_stray_markers}`. For `DedicatedRemoteRequest` keys the
  recipient set is derived from the durable records
  (`IntentJournaled.owner_snapshot ∪ CollateralDiscovered ∪
  RemoteRequestIdentityCaptured.owner`), so the live and recovery paths are
  identical *by construction*; generation keys keep the existing in-memory
  `snapshot ∪ collateral \ delivered` derivation (I9 — legacy V2 attach is
  deliberately not journaled, so journal-derived recipients would be empty
  there). Protocol: terminal CAS → publish → `mark_published` → retire; reopen
  with `SETTLED` and no marker re-publishes then marks.
  `NodeCleanupAggregationV1` gains a doc contract naming
  `(resource_flight_id, owner)` as the consumer idempotence key (no wire
  change — both fields already exist).
- **Red tests:** `crash_after_terminal_before_publication_publishes_once_on_reopen`
  (armed `FailureCountdownV1`-style stop after the terminal append);
  `crash_after_publication_before_marker_redelivers_the_same_idempotence_key`
  (pins the *declared* at-least-once weakening, so a later reader cannot mistake
  it for a defect); `live_and_recovered_recipient_sets_are_identical`;
  `generation_flight_publication_is_unchanged`.
- **Dependencies:** B (retirement ordering).

### Task D — quiescent attempt-start recovery (closes T1)

- **Frozen input:** Task C's commit.
- **Owned:** `crates/bridge-core/src/process.rs`
  (`DurableProcessFlightAttemptV3`), `retained_resource_flight.rs` (registry
  recovery entry point).
- **APIs/types:** `DurableProcessFlightAttemptV3::open_recovered(attempt_id,
  journal, clock, publisher) -> Result<Self, ...>` acquires the **attempt
  lease** (flock on `attempt-<id>.lease`, existing-only, non-blocking; a held
  lease refuses — a second live process must never terminalize the first's
  requests), then runs recovery **once** with `flights` held and asserted
  empty, then sets `recovered: AtomicBool`. `bind_remote_request` refuses
  `Admission("recovery has not run")` unless `recovered`, and **no longer calls
  recovery**. `ResourceFlightRegistryV1::recover_remote_request_reservations`
  becomes `pub(crate)` and takes the held `flights` guard as a parameter so the
  exclusion is enforced by the type system, not by comment.
- **Live-request exclusion proof:** `reserve` takes `flights` *before* the
  durable reservation and holds it through `create`'s `FlightReserved` append
  (`:2176-2202`). Recovery holds the same lock for its whole census. Therefore
  no census can observe or act on any window of a live reservation. Cross-
  process exclusion is the attempt lease.
- **Restart behavior:** on restart, recovery terminalizes/retires per §1.4 and
  publishes; then admission opens. A crash *during* recovery is idempotent —
  every step is CAS or self-healing.
- **Red tests:** `concurrent_successor_cannot_roll_back_a_live_zero_row_reservation`
  and `concurrent_successor_cannot_terminalize_a_live_journaled_intent` (a
  barrier journal decorator parks request A at each cut, drives request B to
  completion, then asserts A is still live and settles `Complete` with its own
  publication); `bind_before_recovery_refuses`;
  `second_process_holding_the_attempt_lease_refuses_recovery`;
  `recovery_runs_exactly_once_per_attempt`.
- **Ripple:** `bridge-api`/`bridge-container`/`bridge-acp` test fixtures move
  from `DurableProcessFlightAttemptV3::new` to `open_recovered`. Keep `new` as a
  deprecated test-only constructor **only if** it refuses `bind_remote_request`
  — otherwise delete it, so no caller can obtain an unrecovered attempt.
- **Dependencies:** B, C.

### Task E — deadline-bounded settlement observation (closes T4)

- **Frozen input:** Task D's commit.
- **Owned:** `retained_resource_flight.rs` (`join_until`), `process.rs`
  (`RemoteRequestSettlementV1`), `bridge-api/src/backend.rs`
  (`cleanup_session_checked` wait only).
- **APIs/types:** `RetainedResourceFlight::join_until(&self, deadline_ms: u64)
  -> Result<SettlementObservationV1, _>` where
  `SettlementObservationV1 { Settled(result), Refused(cause), DeadlineExpired }`,
  implemented with `Condvar::wait_timeout` against the injected
  `MonotonicClock` (recompute `remaining` each iteration; spurious wakeups
  handled by the loop). `join_blocking` is untouched (S14).
  `cleanup_session_checked` becomes `spawn_blocking(move ||
  settlement.join_until(deadline)).await` with **no** `tokio::time::timeout` —
  the worker bounds itself, so the handle always resolves.
- **Red test:** `cleanup_timeout_terminates_its_own_observer` — retain an
  unpolled dispatched scope, run cleanup with a short deadline, assert
  `Unknown` **and** that the blocking worker observably exited (a witness
  `Arc<AtomicBool>` set on the worker's return path, asserted true without
  settling the flight). This fails at `530992b7`, where the worker parks
  forever.
- **Dependencies:** D (ordering only; the API change is additive).

### Task F — checked-cleanup observation and drop-owned custody (closes T2, T3)

- **Frozen input:** Task E's commit.
- **Owned:** `crates/bridge-api/src/backend.rs` only.
- **APIs/types:** `SessionRequestObservationV1` and `RequestDebtV1 { accepted:
  bool, cause: String }` per §1.5; `SessionState` gains
  `admission_in_flight: u32` and `retained_debt: Option<RequestDebtV1>`;
  `RequestAdmission::prepare` takes a ticket in its first session-lock section
  and releases it via a `Drop` guard so **every** exit path (including the `?`
  on `flight.settle(...)`) decrements; `RequestCancelCapability::clear_exact`
  becomes `clear_exact_with_outcome(RequestOutcomeV1::{Proven(result),
  Unproven{accepted, cause}})` and records debt only on `Unproven`;
  `RequestScope::drop` settles **first**, inspects the `Result`, then transfers
  the outcome (so `DurableRemoteRequestFlightV3::drop` becomes the documented
  panic-only backstop, S19); `ApiBackend` overrides
  `forget_session_observed`/`release_session_observed` to record the retained
  debt through the supplied `DiagnosticObserver` as
  `Persistence`/`api.prompt.request_flight`, fatal, with the debt's `accepted`
  flag, before delegating; `process.rs:1019`'s discarded settlement error is
  folded into the returned admission cause (S18).
- **Red tests:** `legacy_active_turn_cleanup_does_not_claim_complete` (**D1**);
  `cleanup_during_the_admission_window_projects_unknown` (barrier between
  durable bind and slot publication); `terminal_refusal_debt_survives_slot_clear`;
  `proven_partial_does_not_taint_a_later_independent_cleanup` (the adjudication's
  refinement — a guard against over-tainting);
  `accepted_drop_with_terminal_refusal_records_persistence_fatal_accepted_true`
  (the exact R3 regression the review demanded).
- **Ripple:** `bridge-coordinator` consumes the disposition unchanged
  (`dispatch.rs:136`); a non-`Complete` value already reaches the session
  manager and still removes the session
  (`session_manager.rs:4403-4451`), so **D1 is observable but does not wedge
  session lifecycle**. `bridge-container::require_complete_cleanup` is on the
  container path, not the API path.
- **Dependencies:** E.

### Task G — reconciliation and carry-forward guards

- **Frozen input:** Task F's commit.
- **Owned:** `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`,
  the slice-3 brief ledger, `docs/reliability-execution-roadmap.md`; one guard
  test module.
- **Content:** reconcile the handoff to the eight closures with per-task line
  counts and gate totals; record **D4** as slice 5's binding idempotence
  obligation; record **D6** (generation-key retirement deferred); record the
  Unix-only V3 journal constraint as a slice-4 arming precondition.
- **Guard tests:** `production_api_route_remains_unarmed` (asserts the spawn
  path assigns `None`); `container_rw_cleanup_composition_is_untouched`
  (asserts the single-field API projection does **not** adopt the two-field
  inner/outer split, and that `BackendCleanupDispositionV1::combine`'s
  `ContainerRw` composition is unchanged) — this is the explicit
  do-not-trigger/do-not-erase check for the 3c1 binding carry-forward.
- **Dependencies:** F.

### Ordering rationale

A (substrate) → B (storage bounds) → C (publication) → D (recovery, consumes B
and C) → E (observation primitive) → F (the only consumer-visible change) → G
(ledger). No intermediate commit arms V3, wraps `ContainerRw`, or makes the
dormant path less safe: A–E are strictly internal to an unarmed component, and
the single production-visible change (D1) lands last but one, alone, with its
own red test and control.

---

## 5. Deterministic adversarial tests and crash/concurrency schedules

**Instruments (all deterministic, no sleeps, no live provider).**

1. *Barrier journal decorator* — a test `ResourceFlightJournal` wrapper that
   parks on a named cut (`AfterReserveBeforeFlightReserved`,
   `AfterIntentBeforeDispatch`, `AfterTerminalBeforeMarker`) until released.
   The pattern already exists (`reaper.rs:1262` decorator,
   `backend.rs::BreakJournalBetweenRounds`).
2. *Crash-cut construction* — build the exact durable prefix by driving the
   real API to the cut and then dropping the handle without settling, or by
   writing the rows directly; reopen a fresh journal + attempt. Established by
   `durable_generation_reservation_recovers_dead_journaled_flight_as_unknown_without_reminting`.
3. *Armed fault countdown* — reuse `FailureCountdownV1`'s shape for
   "fail/stop on the Nth call" on marker write, dir sync, and unlink.
4. *Manual `MonotonicClock`* — already used by the 3a deadline-transfer tests.
5. *Worker-exit witness* — `Arc<AtomicBool>` flipped on the blocking waiter's
   return path.

**Schedules.**

| # | Schedule | Expected |
| --- | --- | --- |
| C1 | A parks at `AfterReserveBeforeFlightReserved`; B runs a full request; release A | A's reservation intact, A settles `Complete`, A publishes once (fails today: B's census rolls A back) |
| C2 | A parks at `AfterIntentBeforeDispatch`; B runs a full request; release A | A settles `Complete` (fails today: B terminal-CASes A `Unknown`) |
| C3 | Two attempts on one journal root | second `open_recovered` refuses on the lease; no terminalization |
| C4 | `bind_remote_request` before recovery | refuses; no reservation created |
| K1–K3 | Reopen at `RESERVED` / `IDENTIFIED` / `INTENT` | `Failed` with the exact recipient set, then retired; repeat reopen is a no-op |
| K4 | Reopen at `DISPATCHED` | `Unknown`, published once, retired |
| K5 | Stop after terminal CAS, before marker; reopen | exactly one publication in total |
| K6 | Stop after publish, before marker; reopen | second delivery with an identical `(resource_flight_id, owner)` key — pins the declared weakening |
| K7 | Stop mid-retirement (each of three cuts); reopen | converges to retired, no orphan file, no second publication |
| P1 | Injected limit 2, third request | refuses before reservation creation, zero POSTs, zero new files |
| P2 | Drive past the limit, then retire, then admit | admission recovers (fails today: permanent `Full`) |
| F1 | Root removed between probe and lock | refuses; root is not recreated |
| F2 | Root renamed aside and replaced after open | every operation refuses; the replacement directory is untouched |
| L1 | Legacy active turn + `release_session_checked` | `Unknown` (D1) |
| L2 | Cleanup inside the admission window | `Unknown` |
| L3 | Terminal refusal clears the slot, then cleanup | `Unknown` |
| L4 | Round settles proven `Partial`, turn ends, then cleanup | `Complete` (over-tainting guard) |
| L5 | Accepted stream dropped + terminal journal refusal, then `release_session_observed` | cleanup `Unknown`; observed diagnostic `Persistence`, fatal, `accepted = true` |
| T1w | Unpolled dispatched scope + cleanup at a short deadline | `Unknown` **and** worker-exit witness true |

**Mutation sensitivity to assert explicitly** (each named in the task's commit
message): removing the `flights`-held requirement from recovery re-reds C1/C2;
removing the population check re-reds P1; removing the marker re-reds K5;
removing the outcome from `clear_exact` re-reds L3/L5; reverting to
`tokio::time::timeout` re-reds T1w; reverting to a path-based root re-reds F2.

---

## 6. Owner decisions, residual risk, unsalvageability criteria

### Decisions requiring owner judgment

- **D1 — `LegacyActive` cleanup projects `Unknown` (recommend: yes).** This is
  the only production-observable behavior change in the whole design: a
  `kind=api` warm session force-released mid-turn will report `unknown` instead
  of `complete` in detached-cleanup projection and terminal evidence. It does
  **not** wedge session lifecycle (the session is still removed;
  only `Err` creates a cleanup-retry tombstone). Alternative: keep `Complete`
  for Legacy and scope honesty to V3 — rejected here because the adjudication
  confirmed it as a blocker and the claim is unprovable either way.
- **D2 — pre-dispatch recovery disposition `Failed` vs uniform `Unknown`
  (recommend: `Failed`).** Provable: `DispatchStarted` is journaled before the
  POST future exists. Affects node collateral rollup (`Partial` vs `Unknown`).
- **D3 — cleanup observation deadline (recommend: a dedicated constant, not
  `cfg.request_timeout`).** 120 s of a blocking-pool thread per cleanup is a
  real resource cost; ~5 s is enough to observe an already-terminal flight.
- **D4 — accept the publication weakening (recommend: yes) and bind slice 5.**
  Exactly-once *call* is removed; exactly-once *effect* requires slice 5's
  `NodeCleanupRecordV2.collateral` writer to be idempotent on
  `(resource_flight_id, owner)`. Without this ruling, T7 cannot be closed at
  all — only relocated.
- **D5 — retirement deletes the flight journal (recommend: yes).** Forensic
  loss for completed requests; the alternative is a second bounded population.
- **D6 — generation keys (ACP/container) keep unbounded reservations for now
  (recommend: defer, ledger).** Same class as T6, but population is
  per-generation, not per-round.

### Residual SMELL / DEFER

- The bounded blocking waiter still occupies a blocking-pool thread until the
  deadline (mitigated by D3, not eliminated).
- Retirement removes an id from `flights`, so `IdentityCollision` no longer
  fires for a *retired* id — cryptographically irrelevant for 32-byte CSPRNG
  ids, stated rather than defended.
- The census is an O(n) directory scan; after Task D it runs once per attempt
  instead of once per request, which also retires risk R3-3.
- `DedicatedRemoteRequestIdV1::parse` accepts the bounded legacy opaque shape,
  so a parsed (non-minted) id does not inherit the full-exposure guarantee. The
  doc says so; nothing enforces it at the diagnostic boundary.
- The V3 file journal becomes Unix-only by construction. Harmless today (all
  callers are Unix-gated tests; Windows CI builds only `bridge-store`), but it
  is a slice-4 arming precondition and must be in the roadmap.
- 3d stays blocked until 3c2 lands, per the adjudication's binding next state.

### What would make the preserved artifact genuinely unsalvageable

None of these hold on inspection; they are the standing criteria:

1. A confirmed WRONG that cannot be fixed without changing
   `RESOURCE_FLIGHT_JOURNAL_SCHEMA_V1` or a landed golden — would invalidate
   3a/3b1/3b2/3c1 wire evidence. *(Does not hold: no new rows, no field
   changes; publication state lives in a sibling marker child.)*
2. A fix that forces `ResourceFlightJournal`'s semantics to change for
   already-landed generation-flight consumers. *(Does not hold: every change is
   discriminated on `ResourceFlightKeyV1::DedicatedRemoteRequest`, and
   recipient derivation is specialized precisely because legacy V2 attach is
   deliberately unjournaled.)*
3. Two or more confirmed WRONGs that cannot be separated into independently
   compiling commits. *(Does not hold: the eight fall into five disjoint
   ownership regions with one strict dependency chain.)*
4. A minimal closure whose total exceeds the slice's stop threshold in a single
   indivisible task. *(Does not hold: the largest task is ~500 implementation
   lines; total is roughly the size of the existing artifact, but split seven
   ways.)*
5. The cancellation/identity core itself being wrong. *(Does not hold: S4/S6/S8
   are KEEP with mutation-sensitive coverage; not one of the eight WRONGs
   touches request identity, the turn epoch, the ABA fence, or the acceptance
   barrier.)*

---

## Verification

Per task: focused gates, then `cargo test --workspace --locked` with totals
reported and the ignored population named (the declared authenticated/live
lane, incl. Kiro and local Ollama). After Task G: `cargo build --release --bin
a2a-bridge --locked`, `cargo deny check`, `./target/release/a2a-bridge validate
--repo-hygiene`, and a same-environment exact-base control for any attributed
failure. No provider, smoke, compatibility, deployment, or running-operator
action; production stays `resource_flight_route_v3 = None`.
