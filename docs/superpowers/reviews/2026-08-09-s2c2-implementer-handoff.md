# R2f1b slice 2c2 — deletion capability — implementer handoff

Date: 2026-08-09. Branch `feat/r2f1b-2c2-deletion` in
`/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s2c2`, base local `main` @ `23909d5c`
(2a + 2b1 + PARKED-1 + 2b2 + 2c1 all folded). Three commits:

| commit | scope |
|---|---|
| `ebfc9c6b` | P1/P2 — the capability, the CAS, the tombstone, `remove_v2` (+ writer and real-git tests) |
| `0f0f734f` | P4/P5/P7 — the drain, the flight's capability branch, durable disposition monotonicity |
| `d25a1f51` | P3/P6 — the tracker extension and the post-loop settlement pass |

This slice closes 2c1's deliberate leak (risk R-3). **No fail-first test stayed red against merged
code**, so nothing is parked as a defect in `main`; one *latent* interaction between preflight and
V3 materialization was found by reading and is reported in §4.3 rather than fixed (it is
unreachable while V3 stays production-unreachable, and fixing it is outside this slice's mandate).

---

## 1. What shipped, per P1–P7

### P1 — `DeleteAuthorized` CAS + `DeletionCapabilityV1` (the mint)

| Symbol | File |
|---|---|
| `DeletionCapabilityV1` (private fields, no public constructor, not `Clone`/`Copy`) | `crates/bridge-worktree/src/custody_writer.rs` |
| `DeletionCapabilityV1::revalidate_for_removal` (consumes `self`) → `AuthorizedRemovalV1` | same |
| `AuthorizedRemovalV1` (private field) | same |
| `DeletionAuthorizationV1` + `is_authorized` (`#[must_use]`) | same |
| `WorktreeCustodianV1::authorize_deletion` / `replace_delete_authorized` | same |
| `WorktreeCustodianV1::record_removed` + `RemovalRecordV1` | same |

The CAS runs inside a `WorktreeCustodianV1`, so both cells (publication + custody) are already
held — the writer is the blocking acquirer and every deletion-side caller takes the same
publication cell with the refusing acquirer, so "close deletion admission" is satisfied by
construction exactly as it is for `preserve_after_cancel`.

Four refusals, each structural rather than advisory:

1. **From-state must be exactly `LiveProtected`.** `Preserved` / `PreservationUnknown` /
   `PreservationPrepared` refuse — this is §5.1's monotonicity on the *durable* side. So does
   `DeleteAuthorized`: that is "no re-mint from the stale capability" (P7 boundary 1), and it is
   what makes a crash between the CAS and the removal recovery-owned rather than re-deletable.
2. **`identities_reverify` must pass**, reusing 2c1's predicate, so a swapped object graph is never
   authorized. The record's `worktree` identity is the RETAINED one, not a fresh observation:
   re-observing would re-open the substitution window the reverification just closed.
3. **An ambiguous CAS mints nothing.** Both candidate records are protective and neither is
   `Removed`.
4. **`record_removed` is legal only from `DeleteAuthorized`**, and writes the retained worktree
   identity — the object is gone by then, so a fresh observation would degrade and 2a's
   completeness rule would reject the record outright.

### P2 — `remove_v2` across the provider impls

| Symbol | File |
|---|---|
| `WorktreeProvider::remove_v2` (refusing default) + `supports_capability_removal` (default `false`) | `crates/bridge-worktree/src/provider.rs` |
| `HostGitWorktree::{remove_v2, supports_capability_removal}` | `crates/bridge-worktree/src/host_git.rs` |
| `remove_and_verify` (free fn, **byte-identical move** of `remove`'s body) | same |

**The 11 `WorktreeProvider` impls, enumerated from source on this base** (`grep 'WorktreeProvider
for'`) — the brief's "nine" and the ledger's "ten" both predate landed doubles:

| # | impl | `remove_v2` | why |
|---|---|---|---|
| 1 | `HostGitWorktree` (`host_git.rs`) | **overrides** | the one production impl |
| 2 | `FakeProv` (`backend.rs`) | **overrides** | the V3-capable double; really deletes the target, so every "is the work still on disk" assertion means something |
| 3 | `NonGitProv` (`backend.rs`) | refusing default | proves the non-git preflight refusal, which precedes every custody transition |
| 4 | `SidecarWriteFailProv` (`backend.rs`) | refusing default | sabotages the legacy `.meta.json`, which V3 never writes |
| 5 | `PartialAddFailProv` (`backend.rs`) | refusing default | its checkout never reaches `LiveProtected`, so it can never be authorized |
| 6–8 | `BlockingRemoveProv`, `BlockingProv`, `BlockingProbeProv` (`backend.rs`) | refusing default | V2 concurrency doubles |
| 9 | `CustodyAddErrProv` (`backend.rs`) | refusing default | the add-refusal double |
| 10 | `ProbeProvider` (`workflow_planner.rs`) | refusing default | planner probe, no removal surface |
| 11 | `FakeProvider` (`tests/r2f1b_deletion_gate.rs`) | refusing default | the cross-crate gate suite is about the *gate*, which the capability path does not use |

The signature is the enforcement: `remove_v2` takes no `repo`/`path`, only an `AuthorizedRemovalV1`,
which can only be built by consuming a `DeletionCapabilityV1` through its descriptor revalidation.
**A caller holding a path cannot reach the method at all.** `supports_capability_removal` is a
side-effect-free preflight run BEFORE the CAS, for the same reason 2b2's repair R4 added one for
the add: discovering the refusing default after publishing `DeleteAuthorized` would strand the
checkout recovery-owned for a removal that could never have started.

`remove_and_verify` was proved a verbatim move (`diff` of the old `remove` body against the new
function body: byte-identical), so V2 and the capability path share ONE definition of "the removal
completed" and the §5.1 post-conditions cannot drift.

### P3 — Post-loop mint on all-healthy outcomes (R-9)

| Symbol | File |
|---|---|
| `NodeCleanupState::checkouts`, `WorkflowCheckoutOwnerV1` | `crates/bridge-workflow/src/executor.rs` |
| `WorkflowCleanupTracker::{record_checkout, materialized_checkouts}` | same |
| `workflow_checkout_outcome` (the extracted health predicate) | same |
| the post-loop settlement loop (before `CleanupObserved`) | same |
| `AgentBackend::settle_workflow_checkout_v1`, `WorkflowCheckoutOutcomeV1`, `CheckoutSettlementV1` | `crates/bridge-core/src/ports.rs` |
| `WorktreeBackend::settle_workflow_checkout_v1` | `crates/bridge-worktree/src/backend.rs` |

The tracker was **extended, not duplicated** (R-9): it is already `Arc`, already `Mutex`-interior,
already keyed `BTreeMap<NodeId, …>`, already written from inside node futures, so none of its ~45
call sites needed re-plumbing. `checkouts` is a `Vec`, not an `Option`, because a node's attempts
can be served by *different backend instances* — a transient failure invalidates the registry entry
and the retry resolves a fresh one whose checkout map knows nothing about the previous instance's
retained entry.

Two enumerated record sites, both immediately after a successful configure (the moment a
worktree-backed session's checkout exists): the node loop's, and the preflight's. Recording at the
configure rather than at the node terminal is what makes the pass reach checkouts whose node failed,
was cancelled, or had its session torn down mid-run.

**The executor deliberately does not know whether a checkout exists.** It records every session it
configured; the worktree layer answers `NoCheckoutUnderCustody` for every V2 session and every
backend that owns no checkout. That is the fail-closed direction — a session the executor forgot to
record is a checkout nobody settles, whereas a needless record costs one map lookup.

### Design note 1 — the drain choice (P4)

**Chosen: (a) a defaulted disposition API on `AgentBackend` —
`settle_workflow_checkout_v1(session, WorkflowCheckoutOutcomeV1) -> CheckoutSettlementV1`.**
Option (b) — a capability-bound disposition handle through `NodeTurnCleanup` — is not merely worse,
it is *unable to carry this settlement*, and the reasons are mechanical:

* **`NodeTurnCleanup` is per-node-turn and already consumed.** `cleanup_warm_turn`
  (`executor.rs`) takes it by `Box` and is handed **no backend at all**; the box is moved into
  `on_exit_observed` at that node's exit. The post-loop settlement runs after the LAST node and must
  reach checkouts of nodes that finished long before, so there is no live handle to route through.
* **It has no production impls.** Verified from source on this base: all seven are `#[cfg(test)]`
  — six in `executor.rs` and `WarmNodeCleanup` in `bridge-a2a-inbound/src/server.rs`, which is
  gated `#[cfg(test)]` at its `struct` (`:867`) as well as its `impl` (`:873`). The brief asked me
  to verify before counting it test-only; it is test-only. A drain with zero production
  implementations is not a drain.
* **The composition mechanism is spawn-factory construction.** The production factory constructs
  `WorktreeBackend` directly around its `AcpBackend`. `ContainerRwBackend` is a separate decorator
  constructed around an `AcpBackend`, never around a custody checkout, so it forwards neither
  custody method without dropping a custody call. A future factory that places a wrapper outside
  `WorktreeBackend` must forward both methods. Using a *second, different* mechanism for the
  deletion half would mean two composition invariants to hold instead of one, and the deletion half
  is the one where dropping the call is dangerous in the other direction (a settlement that silently
  no-ops leaks; a settlement that silently forwards to the wrong layer could delete).
* **Cost is zero, as R-5 predicted.** A defaulted addition costs nothing across the ~119
  `AgentBackend` impls; the workspace compiled with zero new impl obligations.

**Why ONE method with an outcome argument rather than `delete_checkout_v1` + the existing
`preserve_checkout_v1`.** The deletion path must be unreachable without asserting global health. A
separate `delete_checkout_v1` is reachable by a caller that never thought about the outcome; here a
caller cannot invoke the settlement at all without naming which world it is in, and every value
other than `GloballyHealthy` carries a preservation reason — so the *shape* of a forgotten,
mistaken, or defaulted call is preservation.

**The 2c1 RE-3 ruling stands, structurally.** `CheckoutDispositionV1::DeleteAuthorized` has exactly
one writer in the workspace (`settle_workflow_checkout_v1`'s raise), and the flight's mint branch
is entered only when the cell's disposition IS that value. `SessionManager` (its eleven direct
`release_session` sites, `ExpiryClaim`'s three entry APIs, the idle reaper), `BindingGuard::Drop`,
`ConfigureAdmission::Drop` and controller retire reach `run_cleanup_flight` with the cell at
`Reclaim`, so the branch is not entered and the gate refuses exactly as in 2c1.

### Design note 2 — the capability shape (P1)

**Unforgeable** decomposes into two independent structural properties, neither of them a
convention:

* **Not constructible outside the minting module.** Every field of `DeletionCapabilityV1` is
  private with no `pub`, so only `custody_writer` can name them; there is no public constructor, no
  `Default`, no `From`. The only expression in the workspace that builds one is inside
  `WorktreeCustodianV1::authorize_deletion`, verified by token sweep (§5 sweep 2). `backend.rs`
  cannot write one down even though it is in the same crate — Rust field privacy is module-scoped,
  not crate-scoped.
* **Not usable twice.** It is neither `Clone` nor `Copy`, and the ONLY route to a provider removal
  is `revalidate_for_removal(self)`, which takes it **by value**. A failed revalidation consumes it
  too, so a refusal cannot be retried into a success; a fresh attempt needs a fresh mint, which
  needs the record to be `LiveProtected`, which a `DeleteAuthorized` record is not. That chain is
  what "no re-mint from the stale capability" means mechanically.

`DeletionAuthorizationV1::Authorized` boxes the capability (clippy `large_enum_variant` against the
two `String` arms). Boxing does not weaken either property: `Box<DeletionCapabilityV1>` is still
neither `Clone` nor `Copy`, and the capability is still moved out of the box by value.

**Where the disposition generation sits.** Deliberately **not inside the capability**. The
capability is bound to the *identity evidence* (`custody_id`, `canonical_source`, `worktree_path`,
the four `MaterializedIdentitiesV1`) and to the *durable* state it CASed; the `(disposition, epoch)`
generation stays on the cell, and the flight re-checks it against its own captured pair immediately
before minting (`WorktreeBackend::deletion_generation_is_current`). The reason is lifetime: the
capability never escapes the function that creates it — mint, revalidate, consume, drop, all inside
one `run_cleanup_flight` branch — so it has no window in which a stored epoch could go stale, while
the cell does. Putting the epoch in the capability would have implied it was meaningful to carry
one around, which is exactly the affordance this design removes.

**A capability is therefore never stored anywhere**: not in the cell, not in the map, not in a
lifecycle field. There is no place from which one could be read back, replayed, or observed.

**The join→mint window and the epoch guard have different jobs.** The join→mint window is closed by
the `LiveProtected` from-state CAS under both custody cells. The epoch guard is belt-and-braces: its
check is not linearized with the mint because the lifecycle lock is released between that check and
the CAS. This is theoretical-only today because no concurrent preservation caller exists after the
post-loop pass; it is ledgered with a trigger: any new concurrent preservation or settlement caller.

### Design note 3 — where the post-loop settlement state lives (P3)

Mirroring 2c1's note-1 discipline — three candidate homes, and the answer is decided by what each
piece *is*:

| state | home | why |
|---|---|---|
| WHICH checkouts the workflow materialized | `NodeCleanupState::checkouts` (the tracker) | R-9 names it, and the reason survives scrutiny: this is per-node, workflow-scoped, write-once-per-owner data written from inside node futures and read exactly once after the loop. The tracker already has all four properties. A `Vec` keyed by `(session, backend instance)` rather than an `Option`, because a retry can resolve a different backend instance and settling only the last would strand the earlier attempt's checkout. |
| the AUTHORITY to delete one | `CleanupLifecycle::{checkout_disposition, disposition_epoch}` (the cell) | Forced, not chosen — same force as 2c1's: the join decision is made in `start_or_join_cleanup`, which is deliberately synchronous, where the map and `CleanupCellState` are unreachable. `DeleteAuthorized` had to join the value the join key already reads, or the wrong-join defect would reappear with three dispositions instead of two. |
| the DURABLE disposition, once the cell is gone | the custody record on disk | The cell is evicted by the reporter on the first `Ok` report, which a gate refusal is. Nothing in memory survives that, so the record is the authority; `durable_checkout_disposition` reads it on every flight and the flight takes the stronger of the two. |
| the capability itself | **nowhere** — a local, mint-to-consume | See design note 2. |

The deliberate non-fusion, in this slice's own terms: the tracker holds *which* checkouts exist,
the cell holds *whether* deletion is authorized, and the record holds *what actually happened*. A
single store would have to answer all three, and the third must survive a process restart while the
first two must not.

### P4 — The drain (executor → worktree layer)

Covered by design note 1. The call site is one loop in `execute`'s stream, between the terminal
outcome's computation and `WorkflowEvent::CleanupObserved`, iterating
`cleanup_tracker.materialized_checkouts()` in deterministic node order. It yields no new workflow
events (the wire is unchanged) and does not `tracker.record(...)`, so `CleanupObserved` is
byte-identical for V2 and for V3.

### P5 — Disposition monotonicity across cell eviction (opus W3, BINDING)

| Symbol | File |
|---|---|
| `WorktreeCustodyStateKindV1::{is_preserving, is_terminal_preservation}` | `crates/bridge-worktree/src/custody.rs` |
| `probe_custody_record_state` | same |
| `durable_checkout_disposition` | `crates/bridge-worktree/src/backend.rs` |
| `CheckoutDispositionV1::DeleteAuthorized` (between `Reclaim` and `Preserve`) | same |
| `WorktreeBackend::deletion_generation_is_current` | same |
| `retain_refused_entry`'s `from_record` arm | same |

**First half — the durable source.** Every flight now re-derives the checkout disposition from the
record beside the checkout and takes `max(cell, record)`. It can only RAISE, never lower. The
mislabelling opus W3 named is closed in both channels it was visible in: the observed teardown code
(`worktree.teardown.preserved` instead of `.retained`) and the `CheckoutRetentionV1` label on the
map entry. `probe_custody_record_state` is deliberately a *content* read, unlike the gate's
presence-only probe, and `None` means "no answer" — absent, unreadable, or undecodable — which
every caller treats as "no additional knowledge", never as "not preserved".

**Second half — the third disposition, and design note 2's warning going live.** `DeleteAuthorized`
joins `CheckoutDispositionV1` between `Reclaim` and `Preserve`. `Preserve` dominating is §5.1's
monotonicity in the in-memory half: `raise_checkout_disposition` only moves upward, so a mint
REQUEST cannot lower a preserved checkout and the flight that would mint is never started for one.
The epoch is what keeps enum equality from becoming accidental across generations, and
`deletion_generation_is_current` is where it earns its keep: `start_or_join_cleanup` reads the
disposition synchronously and the flight then awaits the configure drain, the per-session state
mutex, and the inner teardown, so a `Preserve` raised in that window would otherwise be invisible to
a flight already carrying `DeleteAuthorized`.

The `Ord`/upward-only rule prevents an in-memory downgrade, but the join→mint closure is the
`LiveProtected` from-state CAS under both custody cells. The epoch guard remains belt-and-braces:
its lifecycle-lock check is not linearized with that CAS because the lifecycle lock is released
between them. This is theoretical-only while no concurrent preservation/settlement caller exists
after the post-loop pass; any such caller is the explicit ledger trigger.

### P6 — Disposition of gate-retained context-free deaths (BINDING)

No new mechanism; it falls out of P3 + 2c1's `WtState::Retained`. The pass reaches a checkout whose
session a context-free entry tore down mid-run because (a) the tracker recorded the owner at
configure time and never removes it, and (b) 2c1's retention keeps exactly one in-memory owner for
the checkout, and `settle_workflow_checkout_v1` reads `Ready`, `Retained` AND `Reserving`. Both
global outcomes are driven over that exact shape
(`a_gate_retained_context_free_death_is_settled_by_the_post_loop_pass`): failed → the claimless
checkout finally gets its claim; healthy → it is removed under a capability and the `Retained` entry
is cleared exactly once.

### P7 — Failure boundaries

| # | Boundary | Defined result | Where enforced |
|---|---|---|---|
| 1 | Crash after `DeleteAuthorized`, before removal | **Recovery-owned.** Record stays `DeleteAuthorized` (sweep `Recover`, 2a data); nothing was removed; a second settlement CANNOT re-mint, because the CAS refuses from that from-state | `authorize_deletion`'s from-state check |
| 2 | Git remove failure | **Retriable-by-recovery, protective.** `remove_and_verify` returns `Err`, so `record_removed` is never called; record stays `DeleteAuthorized`; work on disk | `authorize_and_remove_checkout`'s early return |
| 3 | Prune failure | Same as 2 — `removal_is_complete` is a conjunction of `prune_succeeded && target_absent && registration_absent`, so a failed prune is indistinguishable from an incomplete removal by construction | `remove_and_verify` |
| 4 | Post-condition disagreement | Same as 2, reached through the probe rather than a git error. **`Removed` is never recorded over a disagreeing probe** | `remove_and_verify` + the `Err` early return |
| 5 | Parent-sync failure while recording `Removed` | **`RemovedRecordAmbiguous`.** The checkout IS gone (target and registration absence were verified before the tombstone was attempted), so this is not a removal failure; what is unknown is only whether the tombstone landed, and both candidate records are truthful about an absent checkout. The map entry is still cleared — leaving it would wedge the session id forever for a checkout that does not exist — and the ambiguity is logged and surfaced in the settlement outcome | `CapabilityRemovalV1::RemovedRecordAmbiguous` |

**No new transition-table edge was needed or added.** Boundary 2/3/4's "preserve it instead" is not
available — `DeleteAuthorized -> PreservationPrepared` is not in 2a's frozen table — and recovery
ownership is the defined result rather than a workaround. `LEGAL_CUSTODY_TRANSITIONS_V1` is
byte-identical and `the_deletion_edges_were_already_legal_and_no_new_edge_was_added` pins both the
count and the three absent escape edges.

### Also-yours (small)

* **A capability removal reports through 2c1's typed `CleanupReportV1`:** the branch returns
  `CheckoutCleanupDispositionV1::Removed`, so `cleanup_session_observed` publishes the REAL
  `worktree.teardown.released` code. `Retained`/`Preserved` codes are unchanged. Pinned by
  `a_capability_removal_publishes_the_real_removed_teardown_code`, which also asserts neither
  refusal code appears.
* **The `Retained` map-entry lifecycle composes with the removal:** the capability branch clears the
  map with the SAME `still_same` check the V2 removal path uses, including the `Retained` arm, so
  2c1's "removal clears `Retained` once protection lifts" holds for a capability-driven removal —
  no entry mapped forever, and `state.entry = None` prevents a double removal.
* **V2 positive control:** `settle_workflow_checkout_v1` checks the custody discriminator FIRST and
  returns `NoCheckoutUnderCustody` before raising any disposition or starting any flight, so a
  legacy session gets no extra release, no extra probe and no extra cell at workflow end. Pinned by
  `the_workflow_settlement_is_a_no_op_for_a_legacy_checkout` (both outcomes, cell count asserted
  unchanged, and the ordinary V2 teardown still removes afterwards). The whole pre-existing
  `bridge-worktree` suite is otherwise unmodified.
* **The 13 legacy `configure_session` tests stay green UNTOUCHED.** The complete set of REMOVED
  lines in `backend.rs` is ten, all of them import lines, the `raise_checkout_disposition` signature
  and its three call sites, and one `let retention =` binding that became `let from_barrier =`. No
  legacy test line was touched.

---

## 2. Obligation table

Every P-item and every binding ledger row → test name(s) → status. Test crate/module in brackets:
`[w]` = `bridge-worktree::custody_writer`, `[b]` = `bridge-worktree::backend`,
`[h]` = `bridge-worktree::host_git`, `[x]` = `bridge-workflow::executor`.

| # | Binding item (source) | Test(s) | Status |
|---|---|---|---|
| P1 | the CAS transitions exactly `LiveProtected → DeleteAuthorized` and mints the capability | `a_live_checkout_authorizes_deletion_and_mints_its_capability` [w] | DONE |
| P1 | "Preserve beats any mint" is STRUCTURAL, not conventional — durable half | `a_protected_non_live_checkout_can_never_be_authorized_for_deletion` [w] (all three preserving states) | DONE |
| P1 | — in-memory half (`Ord` + upward-only raise) | `a_preserved_checkout_is_never_removed_by_a_later_healthy_settlement` [b] | DONE |
| P1 | the mint REFUSES when identity reverification fails | `a_swapped_object_graph_is_never_authorized_for_deletion` [w] | DONE |
| P1 | the mint refuses when the custody state is not `LiveProtected` | `an_already_authorized_record_refuses_a_second_mint` [w]; the preserving-state test above | DONE |
| P1 | unforgeable = not constructible outside the minting module | privacy + §5 sweep 2 (one construction expression workspace-wide) | DONE (structural) |
| P1 | unforgeable = consumed by value, exactly once | `revalidate_for_removal(self)`; `an_untouched_capability_revalidates_into_an_authorized_removal` [w] + the swap test | DONE (structural + tests) |
| P2 | `remove_v2` refuses when object identity changed since authorization | `remove_v2_refuses_when_object_identity_changed_since_authorization` [b]; `a_capability_whose_objects_changed_cannot_authorize_a_removal` [w] | DONE |
| P2 | raw-path removal is unreachable without a capability | `raw_path_removal_is_unreachable_without_a_capability` [b] (7 teardown entries, `remove` and `remove_v2` both zero) + the signature itself | DONE |
| P2 | revalidate all four identities immediately before git removal | `revalidate_for_removal` is the only producer of the parameter type; no await between it and the provider call | DONE (structural) |
| P2 | reuse the existing registration + target-absence post-conditions | `remove_and_verify` proved a byte-identical move; `capability_removal_removes_a_real_worktree_and_its_registration` [h] | DONE |
| P2 | then record `Removed` (REPLACE through the custody writer, parent-synced) | `the_removal_tombstone_is_legal_only_from_delete_authorized` [w]; the real-git test above | DONE |
| P2 | refusing default on `WorktreeProvider`, all impls enumerated | the 11-impl table in §1; `supports_capability_removal` preflight | DONE |
| P3 | node-local success cannot remove its checkout | `node_local_success_cannot_remove_its_checkout` [b] | DONE |
| P3 | globally-healthy success removes exactly once | `global_healthy_success_with_capability_removes_exactly_once` [b] | DONE |
| P3 | a completed sibling survives a later workflow failure | `completed_sibling_survives_later_workflow_failure` [b]; `a_completed_node_is_settled_not_healthy_when_a_later_node_fails` [x] | DONE |
| P3 | ANY non-healthy outcome preserves EVERY materialized checkout | `the_global_health_test_requires_every_clause` [x] (truth table); `a_globally_healthy_workflow_settles_every_checkout_as_healthy` [x]; `a_cancelled_workflow_settles_every_checkout_as_cancelled` [x] | DONE for runs reaching the settlement pass; error-exit population LEDGERED (owner: the slice that restructures `execute` exits or activates V3 — slice 3/5) |
| P3 | extend `WorkflowCleanupTracker`, no parallel registry (R-9) | `the_tracker_records_one_owner_per_distinct_session_and_backend` [x] | DONE |
| P4 | the drain reaches the worktree layer from the executor | the [x] settlement tests above, all driven end-to-end through `AgentBackend` | DONE |
| P4 | context-free callers can never reach the mint (2c1 RE-3 stands) | `raw_path_removal_is_unreachable_without_a_capability` [b]; one writer of `DeleteAuthorized` (§5 sweep 2) | DONE |
| P5 | durable disposition survives cell eviction — evict-then-flight | `a_fresh_cell_after_eviction_re_derives_the_preserved_disposition_from_disk` [b] | DONE |
| P5 | — the no-in-memory-evidence order (restart shape) | `a_flight_with_no_in_memory_evidence_still_reports_the_durable_preservation` [b] | DONE |
| P5 | `DeleteAuthorized` joins `CheckoutDispositionV1`; the epoch discriminates | `a_disposition_raised_after_a_flight_started_makes_its_authority_stale` [b] (asserts BOTH the enum and the epoch halves) | DONE |
| P5 | the wrong-join with the third disposition present | `a_deletion_authority_request_never_joins_a_reclaim_flight` [b] | DONE |
| P5 | `Ord` keeps `Preserve` dominant — no downgrade by a later mint request | `a_preserved_checkout_is_never_removed_by_a_later_healthy_settlement` [b] | DONE |
| P6 | the post-loop pass settles a mid-run gate-retained checkout — failed outcome | `a_gate_retained_context_free_death_is_settled_by_the_post_loop_pass` [b], arm A | DONE |
| P6 | — healthy outcome | same test, arm B (removed under capability; `Retained` entry cleared once) | DONE |
| P7-1 | crash after `DeleteAuthorized`: nothing removed, recovery owns it, no re-mint | `a_stranded_authorization_is_recovery_owned_and_never_re_minted` [b]; `an_already_authorized_record_refuses_a_second_mint` [w] | DONE |
| P7-2 | git remove failure: record must NOT say `Removed` | `a_failed_capability_removal_never_records_removed` [b]; `a_removal_that_leaves_the_target_fails_closed_and_forbids_the_tombstone` [h] | DONE |
| P7-3 | prune failure | `cleanup_success_requires_absent_target_registration_and_successful_prune` [h] (pre-existing, now shared by both removal paths via `remove_and_verify`) | DONE (see §4.2) |
| P7-4 | post-condition disagreement never records `Removed` | `a_post_condition_disagreement_never_records_removed` [b]; the [h] test above | DONE |
| P7-5 | parent-sync failure after verified capability removal clears the map but reports typed `RemovedRecordAmbiguous`, never plain `Removed` | `an_ambiguous_tombstone_publication_stays_ambiguous` [w]; `an_ambiguous_removed_tombstone_is_not_reported_as_plain_removed` [b] | DONE (two focused tests; see §2b) |
| small | capability removal reports the REAL removed teardown code | `a_capability_removal_publishes_the_real_removed_teardown_code` [b] | DONE |
| small | `Retained` map entry cleared once by a capability removal, no double removal | `global_healthy_success_with_capability_removes_exactly_once` [b]; the P6 arm-B assertion | DONE |
| small | V2 cleanup byte-identical (positive control at the shared path) | `the_workflow_settlement_is_a_no_op_for_a_legacy_checkout` [b] + the unmodified 203-test base suite | DONE |
| small | the 13 legacy `configure_session` tests stay green UNTOUCHED | the complete removed-line set is 10 lines, none of them in a legacy test | HELD |
| Non-goal | no timers/cutoffs; no flight runners; no serving parity; no V2→V3 terminal-row cutover; no claim exchange; no flock reclamation; no `workload_identity()`; `UnusedSettled` still producerless; **no new transition-table edges** | `the_deletion_edges_were_already_legal_and_no_new_edge_was_added` [w] | HELD (§4.5) |

### Historical mutation checks (pre-repair; all mutations were reverted before this repair round)

| # | Mutation | Expected red | Observed |
|---|---|---|---|
| M1 | disable the flight's capability branch (`if false && …`) | `global_healthy_success_…` | RED (1), and the fall-through was `Retained` — the protective direction |
| M2 | remove the P5 durable re-derivation (all three sites: disposition `max`, report label, retention label) | both P5 order tests | RED (2); observed output was literally `worktree.teardown.retained` for a `preserved` record — opus W3's defect, reproduced |
| M5 | drop the all-nodes-completed clause from `workflow_checkout_outcome` | the truth table | RED (1) |
| M6 | mint skips `identities_reverify` | swap tests, both layers | RED (2) |
| M7 | mint accepts any from-state | preserving-state test, re-mint test, stranded-authorization test | RED (3) |
| M8 | record the tombstone regardless of the removal result | the three P7 boundary tests | RED (3) |
| M9 | rank `DeleteAuthorized` above `Preserve` in the `Ord` | no-downgrade test + epoch test | RED (2) |
| M10 | generation guard compares only the enum, not the epoch | `a_disposition_raised_after_a_flight_started_…` | RED (1) |
| M11 | settlement skips the custody discriminator check | the V2 control | RED (1) |
| M12 | `revalidate_for_removal` always succeeds | `a_capability_whose_objects_changed_…` | RED (1) |

**M9's first attempt was INADMISSIBLE, and is recorded rather than quietly re-run.** The `perl`
one-liner that was supposed to swap the two `Ord` variants silently matched nothing, the suite went
green, and the naive reading would have been "the `Ord` is not load-bearing". The discriminating
observation was that the test contains a direct
`assert!(CheckoutDispositionV1::Preserve > CheckoutDispositionV1::DeleteAuthorized)`, which *must*
fail if the ranking flipped — so a green suite proved the probe, not the code. Re-applied with an
anchored Python edit that verifies its own anchor: 2 RED. A probe that fails for its own reasons
yields no evidence about the hypothesis.

**Historical red-first honesty.** The properties written test-first and observed red before implementation
were the two P5 orders and the P7 tombstone-ambiguity boundary (whose first version armed the wrong
fault seam — a rename fault where a *sync* fault is what produces ambiguity — and was corrected
after observing `Recorded`). Everything else was written implementation-first and is backed by the
mutation evidence above rather than by a recorded red run. One further false start is recorded in
§4.1: a test I wrote to discriminate the "every node completed" clause did not discriminate it, and
the probe that showed why changed the design.

---

## 2b. Repair round RA–RD

This declared repair round is limited to RA–RD; it adds no transition edge and does not alter the
legacy V2 removal path or the 13 `configure_session` tests.

* **RA — inner teardown gates the capability mint.**
  `a_failed_inner_release_skips_the_deletion_mint_and_retains_live_custody` proves a failed release
  invokes zero `remove_v2` calls, leaves the checkout and map entry, returns a non-`Removed`
  settlement, and retains `live_protected` custody.
  `a_failed_inner_release_isolated_to_its_checkout` proves an earlier independent successful
  removal remains exactly once while a later failed release is retained.
* **RB — ambiguous removed record is typed.**
  The existing `fs_custody` parent-sync seam is armed after the authorizing replace, so `remove_v2`
  proves the target absent while the tombstone is ambiguous.
  `RemovedRecordAmbiguous` now crosses the cleanup report, settlement, and observed teardown code
  (`worktree.teardown.removed_record_ambiguous`); map clearing remains unchanged.
* **RC — preservation-unknown settles as preservation.**
  `a_settled_preservation_unknown_is_classified_as_preservation` drives the swapped-identity
  preservation-unknown shape and requires a preserving settlement rather than `Retained`.
* **RD — documentation.** Corrected the production spawn-factory composition explanation, named
  the capability branch as the second inverse lock nesting and its no-cycle basis, and corrected
  this record's settlement-pass/error-exit boundary.

### Repair verification and mutation evidence

The five new RA--RC regressions were written before their behavior changes. With the
environment-provided Cargo cache, all five now pass, and the complete worktree library suite passes
(`236 passed; 0 failed`). The non-test checks also pass:

```text
git diff --check                                                     exit 0
cargo fmt --all -- --check                                           exit 0
CARGO_HOME=/cargo CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets -- -D warnings
                                                                     exit 0
```

The requested six-package test command compiled successfully and reached the integration suites,
but is not green in this environment: `api_entry_resolves_and_serves_through_registry` in
`a2a-bridge` failed while reading the configured agent prompt response
(`PromptStream`, `api.prompt.error_body_read`). This is outside the worktree cleanup path; it does
not affect the focused or complete worktree results above, but it is recorded as a non-green gate
rather than attributed to RA--RD.

### Operator disposition — bridge-verify red tests and the rejected out-of-scope repair (2026-08-09)

This repair round ran through the bridge's own containerized implement flow (gpt-5.6-terra/xhigh —
the dogfood pipeline). Its hermetic Linux verify reported red tests in the `a2a-bridge` bin that
RA–RD does not touch: `owner_admission_lock_release_failure_is_loud_not_silent` (flock LOCK_UN
EBADF), then `staged_candidate_exec_is_bound_to_the_verified_file_object` and
`staged_candidate_nonzero_exit_retains_process_status`. The fix loop responded with production
`compatibility.rs` exec-plumbing changes (an `execveat`/`AT_EMPTY_PATH` launch rework and removal
of a racy post-close descriptor assertion). **The operator REJECTED those changes at the hand-off
boundary as out-of-scope**: a red test against merged code is a defect — or an environment gap —
owned by its own bounded PR, never a repair-round rider. The internal implement-review REJECT
concurred on both grounds (red gate + out-of-scope production changes). The rejected diff remains
inspectable in the implement clone (`impl-96012-qz5j808a`, commit `659e9556`).

**Attribution control (same artifact, host environment, darwin):** with RA–RD applied and
`compatibility.rs` at its merged state, `owner_admission_lock_release_failure_is_loud_not_silent`
passes (1/1) and all four `staged_candidate*` tests pass (4/4) on the host. The container failures
are therefore environmental — the same hermetic-container family (host PID/fd/signal semantics) as
the three `process::` tests the impl config already skips — and are recorded as hermetic-verify
exclusion candidates for the impl config, not as defects in merged code. Caveat carried honestly:
host-pass on darwin does not adjudicate native-Linux behavior, which remains this program's carried
platform exclusion.

All three required mutations were applied, observed red, restored, formatted, and restaged:

| Mutation | Named regression | Observed red |
|---|---|---|
| remove the RA `first_error.is_none()` condition | `a_failed_inner_release_skips_the_deletion_mint_and_retains_live_custody` | 1 failed: settlement became `Removed` |
| map RB `RemovedRecordAmbiguous` to plain `Removed` | `an_ambiguous_removed_tombstone_is_not_reported_as_plain_removed` | 1 failed: typed outcome became `Removed` |
| restore RC `Unknown -> Retained(detail)` | `a_settled_preservation_unknown_is_classified_as_preservation` | 1 failed: settlement became `Retained("AmbiguousCleanup")` |

The first RB probe was syntactically invalid (missing a match-arm comma) and did not reach a test;
it was restored immediately and is inadmissible evidence. The valid RB retry above is the recorded
mutation result.

---

## 3. Historical original-slice gate outputs (pre-repair)

The following gate result predates RA--RD and validates only the original slice. It is retained for provenance, not as a repair-round claim. Run in this worktree at `d25a1f51`, tree clean:

```
git diff --check                                          exit 0, no output
cargo fmt --all -- --check                                exit 0
cargo check --workspace                                   exit 0, zero warnings
cargo clippy --workspace --all-targets -- -D warnings     exit 0, zero warnings
cargo test -p bridge-core -p bridge-worktree -p bridge-coordinator \
           -p bridge-controller -p bridge-workflow -p a2a-bridge
      => 2647 passed; 0 failed; 11 ignored   (exit 0, 50 test binaries)
```

**Per-package deltas.** `bridge-worktree` lib 203 → 231 (+28: 9 in `custody_writer`, 2 in
`host_git`, 17 in `backend`); `bridge-workflow` lib 133 → 138 (+5); `bridge-core` lib 481
unchanged; `bridge-worktree::r2f1b_deletion_gate` 5 unchanged; `a2a-bridge` bin 1081 unchanged.
**+33 net new tests.**

**On the base total — stated as an inference, not a measurement.** I did not re-run the suite
against `23909d5c` in this environment, so the "2614 → 2647" delta is an *inference*, not a
controlled measurement: it rests on the 2c1 handoff's per-package table (worktree lib 203, workflow
lib 133, core lib 481, deletion_gate 5, bin 1081) matching my post-change log everywhere I did not
change a package, plus the arithmetic identity 2647 − 2614 = 33 = 28 + 5. That is consistent but it
is not a same-environment base run. If the orchestrator's fold gate wants an attributable number,
the base run is the control.

**Not run, and why:**

* `cargo test --workspace` — the dispatch brief scopes this slice to the six packages above and
  assigns the workspace suite to the orchestrator at fold.
* `cargo build --release --bin a2a-bridge`, `cargo run -p a2a-bridge -- validate --repo-hygiene`,
  `cargo deny` — not in this slice's gate block.

**Platform exclusions carried forward:**

* **non-unix `dev`/`ino` (risk R-10), and it is now load-bearing in a SECOND place.** 2c1 carried
  it for preservation: off unix `identities_reverify` is always false, so every preservation settles
  `PreservationUnknown{AmbiguousCleanup}`. In this slice the same predicate gates BOTH the mint and
  `revalidate_for_removal`, so **off unix no deletion capability can ever be minted and no
  capability-driven removal can ever run.** The automatic deletion path is therefore entirely
  unavailable there — strictly more protective (checkouts are retained forever rather than deleted
  wrongly), but a real functional loss, and the checkouts accumulate. Named here, untested; darwin
  only.
* `validate_object_identity`'s completeness rule is likewise `#[cfg(unix)]`.
* Linux-arm execution of the custody primitives and real-NFS error-after-effect behaviour remain
  carried from the 2b1 / PARKED-1 / 2c1 ledgers. Nothing here ran against a network filesystem.
* The two real-git tests (`host_git::capability_removal_tests`) require a working `git` on PATH,
  as the pre-existing `host_git` suite already does.

---

## 4. Parked findings, deliberate omissions, declared remainders

### 4.1 The health clause I could not discriminate through the executor, and what I did instead

`workflow_checkout_outcome`'s "every node disposition is `Completed`" clause is §5.1's own
requirement ("including nodes that completed earlier"), and I wrote a test to discriminate it: two
independent nodes, the non-terminal one failing, the terminal one completing. **It did not
discriminate** — the mutation that deletes the clause left it green.

The probe that separated the two candidate explanations (bad regex vs. unreachable state) was to
print the workflow's own events. Result: with two unreferenced nodes, `WorkflowGraph::terminal()`
returns the FIRST unreferenced node, so `terminal_id` resolved to the *failing* node and the run
reported `Terminal { outcome: Failed }`. The clause never fired. And a legal single-terminal graph
cannot produce the shape either: a fan-in makes the terminal depend on the sibling, so a failed
dependency skips the terminal rather than completing it.

**Disposition:** the clause is required by §5.1 and by the fan-out-under-`bounded_independent`
shape a later slice owns, so it stays. It is pinned at the extracted predicate
(`the_global_health_test_requires_every_clause`, one discriminating row per clause, mutation M5
red) rather than through a graph that cannot exercise it. Extracting the predicate was a design
change driven by evidence, and it is recorded as such rather than presented as the original plan.

### 4.2 DECLARED REMAINDER — P7 boundary 3 (prune failure) is proved by conjunction, not by an injected prune fault

`remove_and_verify` refuses unless `prune_succeeded && target_absent && registration_absent`, and
`cleanup_success_requires_absent_target_registration_and_successful_prune` (pre-existing) pins each
conjunct independently, including `removal_is_complete(false, true, true) == false`. Since
`remove_v2` and `remove` are now the same function, a failed prune fails the capability path by
construction. What I did NOT do is inject a real `git worktree prune` failure end-to-end — there is
no seam for it in `run_git`, and adding one is more surface than the boundary warrants. Flagging it
as a coverage choice, not asserting it is equivalent to an injected fault.

### 4.3 PARKED OBSERVATION (not a defect in merged code, and NOT fixed here): preflight and V3 share one frozen target

Found by reading while enumerating the checkout record sites. `ensure_preflight` configures its
session through `attempt_use.configure_session`, i.e. the SAME `BoundSessionSpecV1` and therefore
the same `FrozenWorktreeCustodyPlanV1.target_cwd` as the node's own attempt. Under V3 that means:

1. the preflight session materializes the frozen target and publishes `LiveProtected`;
2. it is then torn down via `cancel_and_forget_preflight_session`, which arms 2c1's barrier with
   `Cancellation`, so the record becomes `Preserved` and the entry becomes `Retained`;
3. the node's own attempt is a DIFFERENT session, so it finds no map entry, reserves, and calls
   `materialize_under_custody`, whose `publish_protection_prepared` is the NO-REPLACE primitive —
   which refuses on the existing record.

So a V3 bound workflow with `preflight = true` would fail its first node configure. **I did not
write a fail-first test for this and did not fix it**, for three reasons: V3 is production-
unreachable through slice 2 by the §5.2 ruling, so nothing can hit it today; the fix is a target
identity or session-scoping decision, not a 2c2 decision; and the standing rule is that a defect in
merged code is its own bounded PR. Recording it so it cannot be discovered the hard way by the
slice that first makes V3 production-reachable (slice 5, per §5.3). My own change makes the
preflight's checkout *settleable* by the post-loop pass, which is strictly better than leaving it
out — it does not create or worsen the interaction above.

### 4.4 SIZE — at the checkpoint, disclosed

Measured against `main`, classifying each added line by whether it falls inside a `#[cfg(test)]
module` in the new file: **1,174 non-test lines added**, **1,578 test lines**, **2,752 total**
across 7 files. 771 of the added lines are comments (doc comments, per house style), so the
non-comment non-test figure is materially smaller than 1,174.

The brief's checkpoint was "stop and report if NON-TEST exceeds ~1,100 or the total heads past
~2,800". **Non-test crossed at ~1,174 — 7% over; the total is under.** The crossing happened in the
last tranche (the executor drain), and the material is not splittable at that point: the settlement
pass, the tracker extension and the health predicate are one obligation, and landing the mint
without a caller would ship an unreachable authority. Reported rather than absorbed.

### 4.5 Deliberate omissions and accepted costs

* **No `CheckoutDispositionV1` value is exposed outside `bridge-worktree`.** The executor names the
  workflow OUTCOME (`GloballyHealthy` / `NotHealthy`), never the disposition. That keeps the
  authority vocabulary inside the layer that owns the checkout.
* **The settlement is not recorded on `WorkflowCleanupTracker`'s cleanup observation**, so
  `WorkflowEvent::CleanupObserved` is byte-identical for V2 and V3. The consequence is that a
  settlement failure is visible only in the log and in the (in-process) `CheckoutSettlementV1`; no
  durable or wire surface reports it. Making it durable is the V2→V3 terminal-row cutover, which is
  slice 5's by the §5.3 ruling.
* **`RemovedRecordAmbiguous` clears the map entry.** The checkout is provably gone, so keeping the
  entry would wedge the session id forever for something that does not exist. The cost is that the
  tombstone's durability is unknown while the in-memory owner is released; the record on disk is
  `DeleteAuthorized` or `Removed`, both truthful about an absent checkout, and neither can lose
  work.
* **The capability path does not consult the 2b1 gate**, by design (see §5, correction 1). It
  substitutes a stronger mutual exclusion — both custody cells held across mint → remove →
  tombstone — for the gate's own probe→removal window. A reader who assumed "everything goes
  through the gate" would be wrong, and that is the single most important thing for a reviewer to
  check.
* **The preflight's checkout is settled with the same outcome as the node's.** On a globally
  healthy workflow the preflight checkout will already be `Preserved` (2c1 arms `Cancellation` at
  that teardown), so the mint refuses and it stays preserved rather than being removed. That is
  protective but it means a healthy V3 run would leave preserved preflight checkouts behind. Named
  as an accepted residual of 2c1's enumerated barrier site, not changed here.
* **`bridge-core` gained types but no tests.** `WorkflowCheckoutOutcomeV1::authorizes_deletion` and
  `CheckoutSettlementV1::removed_the_checkout` are exhaustive-match helpers exercised through the
  worktree and workflow suites; no unit test was added for them alone.
* **No `#[cfg(not(unix))]` behaviour was exercised**, and per §3 the non-unix behaviour is not
  merely untested but *different*: the automatic deletion path is unreachable there.

### 4.6 Non-goals held

No timers or cutoffs (slice 4). No resource-flight runners / claimed non-cancellable materialization
flight (slice 3). No serving parity and no V2→V3 terminal-row cutover (slice 5) —
`NodeCleanupDispositionV1` is untouched and the durable node-terminal row still records `Complete`
for a retained checkout. No claim exchange / `RecoveredLive` production (2d). No `.custody-locks`
flock reclamation. No `workload_identity()` wiring (slice 4). `UnusedSettled` stays producerless.
**No new transition-table edges** — `LEGAL_CUSTODY_TRANSITIONS_V1` is byte-identical, and the
failure boundary that looked like it might want one (`DeleteAuthorized → PreservationPrepared`) was
resolved by defining recovery ownership instead, per the brief's PARK-not-edit rule.

### 4.7 Parked error-exit population

The ordinary post-loop settlement pass is complete for runs that reach it. It does not run on the
approximately 20 `yield Err; return` exits in `execute`: harvest-audit failure, policy
finalization/encoding failure, and invariant-failure exits can be reachable after checkouts were
recorded, with the policy-gated paths precisely the V3 population. The direction remains
protective (`LiveProtected` is sweep-ineligible and recovery-owned); pre-2c2 behavior on these
paths was identical, and V3 is production-unreachable through slice 2. This population is
LEDGERED for the slice that restructures `execute` exits or activates V3 (slice 3/5), alongside
§4.3. The non-discriminating independent-sibling test (§4.1) and inadmissible M9 probe (§2) remain
recorded as draft shortcomings rather than hidden.

---

## 5. §2c SELF-PASS (adversarial, NOT INDEPENDENT)

**Claim under test:** *provider removal, reset, clean, or prune of a custody-discriminated checkout
is reachable ONLY by consuming a `DeletionCapabilityV1` minted through the
`LiveProtected → DeleteAuthorized` CAS from a globally-healthy workflow outcome; the capability
revalidates object identity at use, is consumed at most once, and cannot be minted, replayed, or
forged by any preservation-armed, context-free, or non-healthy path.*

**Refuted by:** any new path invoking a destructive provider/git operation without a valid
capability; a mint reachable from a non-all-healthy outcome or a context-free caller; a capability
usable twice; an identity-changed removal succeeding.

### Search scope

Four sweeps, because any one alone would be an argument from absence.

**1. Token sweep over the added NON-TEST lines** of all seven changed files. Hunk offsets were
tracked into the new files and each added line classified by whether it falls inside a
`#[cfg(test)] mod` region; comment lines were excluded; pattern
`remove_file|remove_dir|.remove(|prune|reset|clean|checkout --|verify_then_remove|unlink|kill|SIGKILL`.
**13 hits, every one accounted for:**

* 6 in `executor.rs` — all the *word* `cleanup` (`WorkflowCleanupDisposition`, `cleanup_tracker`,
  `cleanup_disposition`). No filesystem operation.
* `backend.rs:1820` `map.remove(session.as_str())` — the in-memory `HashMap`, in the capability
  branch's map-clearing block. Not a filesystem call.
* `backend.rs:3064` `.cleanup_session_reported(...)` — the settlement's flight call.
* 5 in `host_git.rs` — all inside `remove_and_verify`, which was proved a **byte-identical move** of
  the pre-existing `HostGitWorktree::remove` body (`diff` of the two bodies is empty). No new
  destructive capability; the same `git worktree remove` + `prune` that already existed, now shared.

Every `std::fs::remove_dir_all` in the diff is inside `#[cfg(test)]` — fixture teardown, plus
`FakeProv::remove_v2`'s deliberate real deletion of its own temp target.

**2. Call-graph read of each new authority path, and a reachability enumeration.** Read
individually rather than inferred from (1):

* `settle_workflow_checkout_v1` — **one production caller workspace-wide**: the post-loop loop in
  `executor.rs`. Its first action is a map read; non-`Protected` returns before any effect.
* `WorkflowCheckoutOutcomeV1::GloballyHealthy` — **one producer workspace-wide**:
  `workflow_checkout_outcome` in `executor.rs`, which is called from exactly one place, after the
  node loop.
* `CheckoutDispositionV1::DeleteAuthorized` — **one writer**: the `raise_checkout_disposition` call
  inside `settle_workflow_checkout_v1`'s healthy arm. Every other occurrence is a read comparison
  or a test.
* `WorktreeCustodianV1::authorize_deletion` — **one production caller**: inside
  `authorize_and_remove_checkout`'s `spawn_blocking`, which is itself called from exactly one place:
  the flight branch guarded by `disposition == DeleteAuthorized` **and**
  `deletion_generation_is_current`.
* `DeletionCapabilityV1` construction — **one expression workspace-wide**, in `authorize_deletion`,
  behind private fields.
* `revalidate_for_removal` — **one production caller**, in `authorize_and_remove_checkout`, with no
  await between it and `provider.remove_v2`.
* `provider.remove_v2` — **one production caller**, the same function. `HostGitWorktree::remove_v2`
  delegates to `remove_and_verify` and does nothing else.
* `record_removed` — **one production caller**, after `remove_v2` returned `Ok`.

`WorktreeProvider` has no `reset`, `clean`, `checkout` or standalone `prune` operation at all; the
only `prune` is inside `remove_and_verify`. That is the structural half of the claim.

**3. The 2b1 gate is byte-identical and still gates.** `checkout_removal_refusal`,
`CheckoutRemovalWindowV1::enter`, `probe_custody_record_presence` and
`CustodyRecordPresenceV1::authorizes_checkout_removal` are unchanged — the ONLY diff line touching
those names is the `use` statement that adds `probe_custody_record_state` beside the existing
import. `WtEntry.custody` is never written back to `Legacy` anywhere in the diff. So every
non-capability path behaves exactly as it did at 2c1, and
`raw_path_removal_is_unreachable_without_a_capability` counts actual provider invocations through a
double across seven teardown entries rather than reading source.

**4. Alternative-mechanism check (evidence admissibility).** What else would produce the same "no
unauthorized destructive call" output? (a) A destructive call reached through an existing symbol
whose name is outside the token set — discriminated by sweep 2's call-graph read and by the
behavioural counters in `raw_path_removal_is_unreachable_without_a_capability`, which count `remove`
and `remove_v2` invocations separately (a single counter could not distinguish "removed with
authority" from "removed by the raw-path call"). (b) A test suite in which the provider never
actually deletes, making every "the work survives" assertion vacuous — discriminated by
`FakeProv::remove_v2` really deleting its target, by `global_healthy_success_…` asserting
`!Path::new(&target).exists()`, and by the two real-git tests in `host_git`.

### Verdict: **SURVIVED — with three corrections left visible.**

**Correction 1 (the gate is NOT the mechanism on the capability path, and this is the load-bearing
one).** A reader who takes the claim to mean "the 2b1 fail-closed gate is what stops unauthorized
deletion everywhere" would be wrong for the capability branch: that branch runs *before* the gate
block and returns without reaching it. What substitutes for the gate there is stronger, not weaker
— the `WorktreeCustodianV1` is alive across mint → `remove_v2` → tombstone, so BOTH custody cells
are held for the whole window, and every deletion-side caller (which takes the publication cell with
the refusing acquirer) fails closed against it; the gate's own window covers only probe → removal.
But it is a *different* mechanism, it is the one thing in this slice that could turn a bug into a
deletion, and it should be read as the primary review target. Any outcome that did not remove the
checkout falls through to the unchanged gate, which refuses on the custody evidence exactly as
before.

**Correction 2 (scope of "cannot be minted by a non-healthy path").** The claim holds for every
path in this workspace *today*, and the proof is a reachability enumeration (sweep 2), which is a
whole-program property and therefore only as good as the search. It is not enforced by a type: a
future caller could construct `WorkflowCheckoutOutcomeV1::GloballyHealthy` — the enum is `pub` and
its variant is constructible anywhere — and call `settle_workflow_checkout_v1`. What that caller
still could not do is mint over a preserved or already-authorized record, or over a swapped object
graph. The accurate statement is: *the outcome argument is a `pub` value whose only producer is the
post-loop health predicate; the durable from-state check and the identity reverification are the
properties that hold regardless of who asks.*

**Correction 3 (what "consumed at most once" does and does not cover).** The capability is consumed
by value and never stored, so a single capability cannot drive two removals — that is structural.
It does NOT say a checkout can be authorized only once *in total*: a mint whose flight then crashed
leaves `DeleteAuthorized` on disk, and a second settlement will *attempt* a second mint. That
attempt refuses (`an_already_authorized_record_refuses_a_second_mint`,
`a_stranded_authorization_is_recovery_owned_and_never_re_minted`), so the effect is the same, but
the mechanism is the from-state check rather than the type system, and it is the mechanism a
reviewer should test rather than the phrase.

**Also checked and cleared, for the record:** the capability branch takes both custody cells with
the *blocking* acquirers while `cell.state` (the per-session single-flight async mutex) is held.
That cannot deadlock, by the same argument 2c1 recorded for the barrier: the only other holders of a
checkout's publication cell are the V3 writer (which never takes `cell.state`) and refusing
acquirers (the gate and both sweep arms), which never wait. The capability branch releases both
cells before the gate's own refusing acquisition further down, and on the arm where it returns early
the gate is never entered at all.
