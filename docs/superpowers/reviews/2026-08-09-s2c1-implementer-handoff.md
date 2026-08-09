# R2f1b slice 2c1 — fail-closed preservation — implementer handoff

Date: 2026-08-09. Branch `feat/r2f1b-2c1-preservation` in
`/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s2c1`, base local `main` @ `a9962e25`
(2a + 2b1 + the PARKED-1 publication-classification fix + 2b2). Three commits:

| commit | scope |
|---|---|
| `6c432097` | P1–P7 production code |
| `3d47d4af` | writer + backend test surface |
| `161e7ac0` | executor barrier witnesses + the flight-side placement test |
| `884ab1f3` | first handoff |
| *(repair round)* | RA `PreservationPrepared` resume + RB locator downgrade; RD reservation-side custody evidence + RE-6 label split; RC cold non-success inventory; RE docs/rulings |

**Review status.** Dual-lens adjudicated: opus REVISE (2 WRONG/BLOCKER + 1 WRONG/DEFER + 10 SMELL),
sol REJECT (7 BLOCKERs), several sol items re-scoped or refuted in adjudication. One declared repair
round, folded below as RA–RE. The preservation-only invariant SURVIVED both lenses (opus verified
the narrowed "while custody evidence exists" phrasing), the P5 custody-positive split was ruled
CORRECT in both directions, and the P3 join key was ruled correct as a key — the repairs are about
completeness and claim-truthfulness, not the safety direction. **Pushback: none.** Every item
reproduced against source; two were sharper than the summary and I widened the fix accordingly (see
RB and RE-6).

---

## 1. What shipped, per P1–P7

### P1 — Preservation transitions

| Symbol | File |
|---|---|
| `WorktreeCustodianV1::preserve_after_cancel` | `crates/bridge-worktree/src/custody_writer.rs` |
| `WorktreeCustodianV1::{replace_preservation_prepared, replace_preserved, replace_preserving_state}` | same |
| `WorktreeCustodianV1::{current_state_kind, identities_reverify, binding}` | same |
| `PreservationOutcomeV1` + `is_protective` / `is_terminal_preservation` / `From<CustodyWriteRefusalV1>` | same |
| `preserve_entry_checkout` (the backend's async wrapper, `spawn_blocking`) | `crates/bridge-worktree/src/backend.rs` |

`preserve_after_cancel` is §5.1 steps 3–7 for one checkout. Steps 1–2 are satisfied by
construction: the custodian already holds the publication cell (blocking acquirer) and the custody
cell, and holding the publication cell **is** "close deletion admission" — every deletion-side
caller takes the same cell with the refusing acquirer and fails closed while it is held.

The sequence, and the four decisions inside it:

1. **Read the from-state under the held cells.** `Preserved` / `PreservationUnknown` return
   unchanged (§5.7's last row). Any other from-state without a legal edge is `Refused`, so the
   writer never invents an edge outside 2a's frozen table.
2. **Reverify the retained identities BEFORE minting anything** (P7). A mismatch means the object
   graph is not the one we materialized, so the terminal state becomes
   `PreservationUnknown{AmbiguousCleanup}` and the RETAINED identities — not the replacement's —
   are what the claim records.
3. **`LiveProtected → PreservationPrepared → (Preserved | PreservationUnknown)`.** Never a direct
   `LiveProtected → PreservationUnknown`; that edge does not exist and this slice adds none.
4. **Any ambiguous publication stops the sequence.** After an ambiguous replace the from-state is
   genuinely unknown, so a further replace could publish an illegal edge. Both candidate states are
   protective, so the answer is `Ambiguous` and nothing else is written (§5.7 row 5).

Both preserving states carry a claim, because 2a's `claim_presence` makes it `Required` for all
three preserving states. That is settled data, built against, not re-derived. The **ambiguity-discard
hardening** (2b1 opus S-7) is the single `From<CustodyWriteRefusalV1> for PreservationOutcomeV1`
conversion plus `#[must_use]` on the outcome: there is exactly one place an `Ambiguous` write can be
turned into an outcome, so no call site can fold it into "nothing happened" with its own match.

### P2 — Barrier placement (R-8)

| Symbol | File |
|---|---|
| `AgentBackend::preserve_checkout_v1` (defaulted), `CheckoutPreservationV1`, `CheckoutPreservationReasonV1` | `crates/bridge-core/src/ports.rs` |
| `WorktreeBackend::preserve_checkout_v1` | `crates/bridge-worktree/src/backend.rs` |
| the flight-side barrier (banner "R2f1b §5.1 step 6 PRESERVATION BARRIER") in `run_cleanup_flight` | same |
| `preserve_checkout_before_signal` + four "ENUMERATED BARRIER SITE" call sites | `crates/bridge-workflow/src/executor.rs` |

Two halves, and the split is the design:

* **Caller side.** Sites that signal the session *before* they ask for cleanup must preserve first,
  because that signal is what §5.1 orders preservation against. Four enumerated sites, all driven
  by a test: `cancel_and_forget_preflight_session`; the cold prompt-open cancellation arm; the cold
  drain cancellation arm; and the rich-flush-failure destroy — the one cold exit with **no**
  preceding `cancel_observed`, and the only one whose trigger is a node FAILURE rather than a
  cancellation, which is why the reason cannot be inferred inside the backend.
* **Flight side.** `run_cleanup_flight` runs the same barrier before `inner.forget_session_checked`
  / `release_session_checked`, which is the death signal for every remaining R-11 entry
  (`ConfigureAdmission::Drop`, both configure rollbacks, `retire`, forget/release/observed,
  `BindingGuard::Drop`, `ExpiryClaim`'s three entry APIs, the eleven direct `release_session`
  sites, workflow cold cleanup, controller retire). It is idempotent with the caller side, and it
  is what makes a first attempt that returned `Ambiguous` or `Refused` retriable.

The flight-side barrier runs **only under a `Preserve` disposition**, and that is §5.1's own rule,
not an optimization: "a node-local success is NOT a checkout disposition — it settles the node
session but leaves its checkout `LiveProtected` under a workflow-level disposition flight." A
context-free entry (a reaper, a `Drop`) has no workflow outcome to consult, so terminalizing from
one would decide a disposition only 2c2's post-loop mint is entitled to decide. Those entries still
cannot delete: the gate refuses and the report says `Retained`.

`WorktreeBackend` does **not** forward `preserve_checkout_v1` to `inner`. A checkout belongs to the
worktree layer; inner backends own none.

**The default is a no-op answer, not a refusal**, and the reasoning is on the trait method: §5.4's
refusing defaults exist for methods whose silent absence lets an *effect* happen unguarded, and this
method's absence produces no effect at all. The hazard the default does carry is a forwarding
wrapper that neither preserves nor delegates. Verified against source: `WorktreeBackend` is the
outermost production decorator (the agent factory wraps `inner` with it at `main.rs`, and nothing
wraps the result), so no production composition can drop the call. A future wrapper placed OUTSIDE
it must forward — stated on the method.

### P3 — Single-flight disposition keying

| Symbol | File |
|---|---|
| `CheckoutDispositionV1` (`Reclaim` < `Preserve`) | `crates/bridge-worktree/src/backend.rs` |
| `CleanupFlightSlot::{disposition, disposition_epoch}` | same |
| `CleanupLifecycle::{checkout_disposition, disposition_epoch, preservation_reason}` | same |
| `WorktreeBackend::raise_checkout_disposition`, `next_checkout_disposition` | same |

Join rule: a flight may be joined only by a request of the **same disposition generation**
(`existing.disposition == disposition && existing.disposition_epoch == disposition_epoch`), on top
of the pre-existing strength ordering. A request never *sets* the cell's disposition inside
`start_or_join_cleanup`; it reads it. Only `raise_checkout_disposition` writes it, and only upward,
which is §5.1's monotonicity in the key: once a checkout's disposition is preservation, a later
equal-strength reclaim is READ as preservation and joins the preserve flight rather than minting a
removal.

The failed-configure reporter's re-spawn re-reads the cell rather than reusing its captured value,
because a preservation raised while the retry was sleeping must govern the next attempt — that loop
is the one path that re-spawns a flight with no caller.

### Design note 1 — where the retained disposition state lives (P4/P5)

Three candidates were on the table: a new `WtState` variant, cell-resident state, or a map sidecar.
The answer is **all three, split by what each piece actually is**, and the split is load-bearing:

| state | home | why |
|---|---|---|
| retained OWNERSHIP + reusability | `WtState::Retained { entry, retention }` (the map) | The defect (D-2) is that the map's owner is *lost*; the fix has to be a map entry. It must not be `Ready`, because `Ready` means reusable (opus S-3) — so both configure entry points refuse it by name, and P6 falls out of P5's state rather than needing a second mechanism. One authoritative copy, so no two-writer reconciliation. |
| the monotonic DISPOSITION | `CleanupLifecycle` (the cell) | Forced by the code, not chosen: the join decision is made in `start_or_join_cleanup`, which is deliberately synchronous (it publishes the flight before the caller's first await, so dropping the report receiver detaches rather than cancels). The map is behind an async mutex and is unreachable there; `CleanupCellState` is too. `CleanupLifecycle` is the one `StdMutex` on the join path. |
| materialization EVIDENCE | `WtEntry.protection: Option<Box<ProtectedCheckoutV1>>` | It is immutable for the checkout's lifetime, and it must travel with the value that reaches the cleanup flight (`state.entry`), which is a `WtEntry` clone. Putting mutable disposition here instead would need reconciling the map's copy with the cell's clone. |

The deliberate non-fusion: `WtEntry.custody` (`Legacy` / `Protected`) stayed a separate field from
`WtEntry.protection` rather than becoming `Protected(Box<…>)`. They answer different questions —
`custody` is the AUTHORITY question the gate asks ("may this be deleted?"), `protection` is the
EVIDENCE question the barrier asks ("can a truthful claim be minted?") — and fusing them would make
"protected, but its identities were never captured" unrepresentable. That state is real: it is
exactly what 2b1's `the_discriminator_alone_refuses_deletion_with_no_record_on_disk` exercises, and
the fail-closed answer there is *refuse the deletion AND refuse to mint a claim*, which requires
both fields. Pinned by `a_protected_entry_with_no_retained_identities_refuses_to_mint_a_claim`.

### Design note 2 — the disposition-identity shape in the flight key (P3)

The identity is **a monotonic `(CheckoutDispositionV1, u64 epoch)` pair, stamped on the cell when
the disposition CHANGES and copied onto each flight slot**, not a per-request nonce and not a bare
enum.

* **Not a per-request nonce.** Every request would then have a unique identity and nothing could
  ever join, which destroys the single-flight property the whole cell exists for. The identity has
  to name the *generation of the disposition*, not the request.
* **Not a bare enum.** Today `disposition` alone would suffice — only a strict upgrade mints a new
  epoch, so equal dispositions imply equal epochs. The epoch is carried anyway for two reasons: it
  makes "a flight serves exactly the disposition generation it was started for" an asserted
  invariant rather than an inferred one, and a future third disposition (2c2's `DeleteAuthorized`
  is the obvious candidate) would make enum equality accidentally true across generations. The
  monotonic counter is backend-wide rather than per cell so the ordering between two sessions'
  disposition changes is total, which is what makes a wrong-join reproducible in a test rather than
  merely improbable.
* **`Ord` with `Preserve` dominating** is where §5.1's "no later healthy projection or TTL can mint
  deletion authority" lives. A later reclaim cannot downgrade; it is read as preservation.

### P4 — Typed retained/refused cleanup disposition (2b1 sol-1, BINDING)

| Symbol | File |
|---|---|
| `CheckoutCleanupDispositionV1` + `completed_code` | `crates/bridge-worktree/src/backend.rs` |
| `CleanupReportV1`, `CleanupReportReceiver`, `wait_for_cleanup_report`, `cleanup_session_reported` | same |
| the teardown codes `worktree.teardown.retained` / `worktree.teardown.preserved` | same |

`run_cleanup_flight` now returns a `CleanupReportV1 { result, checkout }`. `Removed` is the only arm
that means the checkout is gone; `Retained` and `Preserved` are the refusal arms; `RemovalFailed`
and `NotNeeded` cover the rest. `cleanup_session_observed` publishes the disposition's own terminal
code, so a refusal no longer emits the byte-identical `worktree.teardown.released` a real removal
emits.

`PhaseStatus` stays `Completed` rather than `Skipped`, deliberately: the session teardown genuinely
did complete (the refusal is scoped to the checkout, and a real inner failure is still `Failed`), so
`Skipped` would assert that no teardown happened. What was skipped is the checkout removal, and the
code names exactly that.

The failed-configure retry contract is intact: refusal still reports `Ok`, so the reporter loop does
not spin. `cleanup_session_with_sealed_admission` is now the result-only projection of
`cleanup_session_reported`, which is why the ~40 existing call sites are untouched.

**Declared remainder — see §4.1.** The `NodeCleanupDispositionV1`-level projection and the
session-manager owner bookkeeping are NOT shipped, with reasons.

### P5 — Ownership retention through refusal (2b1 sol-2, BINDING)

| Symbol | File |
|---|---|
| `WtState::Retained`, `CheckoutRetentionV1` | `crates/bridge-worktree/src/backend.rs` |
| `WorktreeBackend::retain_refused_entry` | same |
| `entry_for_cleanup`'s `Retained` arm (clone, never pop) | same |
| the `still_same` map-removal check, extended to `Retained` | same |

On a custody-positive refusal the entry is re-inserted (`Reserving`, popped by `entry_for_cleanup`)
or promoted (`Ready`, only when a terminal preservation exists) as `Retained`. `entry_for_cleanup`
clones a `Retained` entry rather than popping it, exactly like `Ready`, so it survives repeated
refusals; the successful-removal path matches `Retained` alongside `Ready` so the entry is cleared
once protection lifts — without that arm the removal would succeed and the entry would stay mapped
forever, which is a *different* leak from the one `Retained` fixes.

**Scoped to the two custody-positive refusals** (`Discriminated`, `RecordPresent`), and this is a
deliberate narrowing with a stated reason. `ProbeInconclusive` and `CellContended` are transient
unknowns, not evidence that a custody record governs the checkout, and 2b1's accepted V2 trade for
them is a *self-healing* protective leak: the legacy `.meta.json` is retained, so the run-end guard
or the next boot sweep reclaims it and a configure retry proceeds. Converting an unknown into a
durable non-reusable retention would turn that self-healing leak into a permanently wedged session
id after one transient `EACCES` on the worktree root — strictly worse, and justified by no custody
evidence. Pinned by `an_inconclusive_probe_refusal_does_not_retain_the_entry`.

### P6 — Ready-reuse policy (2b1 opus S-3, BINDING)

Enforced by an explicit `Some(WtState::Retained { .. })` arm in BOTH configure entry points
(`configure_bound_resolved_with_admission` and `configure_session`), which the compiler demanded
the moment `WtState` gained a variant — the policy could not be forgotten. A retained checkout is
refused as a session cwd with a `BridgeError::ConfigInvalid` naming the retention.

**V2 reuse is byte-identical, structurally.** A legacy checkout never enters `Retained`: the state
is reachable only from a custody-positive gate refusal or the preservation barrier, and the `Ready`
arm below is unchanged for both regimes, with no probe added. Pinned by
`a_legacy_ready_entry_is_still_reused_exactly_as_before`.

A `LiveProtected` V3 checkout stays `Ready` and reusable, which is §5.1's rule; only a terminal
preservation (awaiting R2f2) is un-reusable.

### P7 — Success-path identity retention (2b2 opus S-9 / sol S-3, BINDING)

`materialize_under_custody`'s `Materialized` arm now retains the four descriptor-observed identities
and the add's recovery locator in `ProtectedCheckoutV1`, instead of discarding them because
`LiveProtected` forbids a claim. `preserve_after_cancel` reverifies them by descriptor
(`identities_reverify`) at claim-mint time and refuses to claim a replacement.

Two decisions recorded on the code:

* **The reason for a failed reverification is `AmbiguousCleanup`.** Of the six frozen
  `PreservationReasonV1` values it is the only one that describes "a cleanup-time decision where the
  protective reading was taken". The cost is that the *trigger* (node failure vs cancellation) is
  not preserved in the record on that arm — named as a remainder in §4.4.
* **The locator is the materialization-time answer, downgraded on mismatch.** No `WorktreeProvider`
  operation exposes git-level registration, so nothing re-probes it at preservation time. Recording
  the add's proven answer and downgrading to `RegistrationUnproven` whenever the object graph no
  longer verifies is the honest shape, and it gives `RecoveryLocatorV1::RegisteredWorktree` its
  first producer.

A degraded re-observation never matches a complete retained identity, so a **vanished** object fails
verification rather than silently passing as "same path".

### Also-yours (small)

* **`before_rename` crash injection for the REPLACE path (2b1 opus S-8): ALREADY PRESENT, not
  re-added.** The PARKED-1 fold generalized the seam to both primitives:
  `PublicationRenameFaultV1::{BeforeEffect, AfterEffect, UnlinkSourceOnly}` +
  `fail_publication_rename_on_nth_call_for_test`, one countdown shared by publish and replace. This
  slice consumes it — `preservation_publishes_prepared_before_its_terminal_state` arms
  `BeforeEffect` on rename 2 of the preservation sequence. Verified against source before deciding
  not to add a second seam.
* **Ambiguity-discard hardening at this slice's call sites (2b1 opus S-7):** the single
  `From<CustodyWriteRefusalV1>` conversion described under P1, plus `#[must_use]` on
  `PreservationOutcomeV1` with a message naming the hazard. The `CustodyPublicationV1` enum was not
  redesigned.
* **V2 positive control:** the whole pre-existing `bridge-worktree` suite is unmodified and green,
  including the 13 legacy `configure_session` tests, plus three purpose-built V2 controls
  (`the_preservation_barrier_is_a_no_op_for_a_legacy_checkout`,
  `a_legacy_ready_entry_is_still_reused_exactly_as_before`, and the `released`-code arm of the
  projection test).

---

## 1b. Repair round (one declared round; RA–RE)

| Item | Fix | Red-first / mutation evidence |
|---|---|---|
| **RA** `PreservationPrepared` must resume (opus W2 / sol B-1) | `preserve_after_cancel` gained a `PreservationPrepared` from-state arm that RESUMES straight to the terminal step, skipping the prepared re-publish (`PreservationPrepared → PreservationPrepared` is not an edge in the frozen table). The resume **re-derives `verified` from the live objects and never reads the stranded record's claim back** — that claim was minted at prepare time and laundering it into a terminal one is exactly P7's hazard through a path P7's own test does not cover. | `a_stranded_prepared_record_resumes_to_exactly_one_terminal_state` (the fault is armed at rename **2**, so a resume that re-published prepared would consume it and re-strand — the rename count is the discriminator) and `a_resume_reverifies_the_live_objects_and_never_trusts_the_stranded_claim`. Mutations: re-publish prepared → RED; restore the old refusal → both RED. |
| **RB** locator downgrade (opus W1 / sol B-7) | `recovery_locator` is now rewritten to `RegistrationUnproven` whenever `verified == false`. **Widened beyond the ask:** applied to the PREPARED publication as well as the terminal one. `verified` is known before either is written, so publishing a confident locator and contradicting it one rename later would leave a crash window in which the durable record is more confident than the writer ever was. | The existing swap test now asserts `claim.recovery_locator`; new `a_vanished_object_also_downgrades_the_claims_recovery_locator` covers the degraded-re-observation branch, which travels a different path through `identities_reverify` than a swap. Mutation: pass the locator through → both RED. |
| **RC** arm `Preserve` across the cold inventory (sol B-2) | New `ColdExitV1 { Success, Failure, Cancellation }` + `preservation_reason()` is now the ONE outcome→disposition decision, with `preserve_then_cleanup_cold_session` for sites whose first signal is the cleanup. **13 sites converted** (preflight: cancel-during-configure, configure error, cancel-after-configure, prompt-open-not-accepted; node loop: configure transient/fatal, cancel-after-configure, attestation-request failure, prompt transient/fatal, empty final, stream transient/fatal) plus **2 computed** (the node loop's shared success/failure teardown, and the preflight outcome fork — both compute the exit before the teardown). The four originally enumerated sites keep their explicit barrier ahead of their own `cancel_observed`, so they do not double-arm and the ordering vectors stay exact. **The code comment states the consequence honestly:** this is recovery-evidence quality, NOT a deletion or overwrite hole — the gate refused those removals anyway and `Retained` refuses reuse; what was lost was the exact claim. | `the_cold_exit_disposition_mapping_preserves_only_non_success_exits` (table, incl. `Success => None`), `preservation_precedes_the_teardown_at_the_configure_failure_exit`, `..._at_the_prompt_failure_exit`, `preservation_precedes_every_transient_attempts_teardown`, plus the pre-existing success control. Mutation: `Failure => None` → all 4 RED. |
| **RD** custody evidence survives inner-configure failure (sol B-4 / opus S2, bounded form) | The `Reserving` map entry is upgraded with `WtCustodyV1::Protected` + `ProtectedCheckoutV1` **immediately after `materialize_under_custody` returns, before the next await**. Previously the evidence landed only at the `Ready` publication, which the failure and cancellation arms never reach, so a rolled-back materialization left a `Legacy`/`None` entry beside a durable `LiveProtected` record and no exact claim was ever mintable again. | `custody_evidence_survives_an_inner_configure_failure_after_materialization` and `custody_evidence_survives_a_cancelled_configure_after_materialization`, both driving a new `ConfigureFailInner` double whose BOUND configure honours `fail_configure` and the blocking gate (`FakeInner`'s honours neither, which is why these arms were unreachable). Mutation: move the upgrade back to the `Ready` site → both RED. |
| **RE-6** ambiguous retention label (opus S8) | `CheckoutRetentionV1::PreservationAmbiguous` split out from `PreservationUnknown`: after an ambiguous PREPARED publication the disk says `PreservationPrepared`, so the old label asserted a terminal state that is not there — and with RA landed that state is resumable, which is precisely why it must stay distinguishable. Both mapping sites updated. | Covered by the type split + the existing ambiguity tests; no behaviour change beyond the label. |
| **RE-6b** keep-strongest comment vs code | `CheckoutRetentionV1` now derives `Ord` and `retain_refused_entry` actually upgrades a weaker recorded retention. The comment previously claimed this and the code did nothing. | Made true rather than reworded. |
| **RE-3** context-free-caller RULING | Recorded in the doc comment on `WorktreeBackend::preserve_checkout_v1`: `SessionManager`, both `Drop`s, the reaper and controller retire must NOT arm `Preserve`, and none does. Refutes "preserve before every manager cancel" with the mechanism — an unconditional manager-side `Preserve` would terminalize a healthy warm session that merely went quiet, and `Preserved` is R2f1b-terminal. | Doc + the pre-existing `an_ordinary_teardown_leaves_a_live_checkout_live_and_still_undeletable`. |
| **RE-7** `reconcile_config` (opus S7) | Explicit DECISION comment, not code: `Retained` deliberately has no arm and falls through to "not mapped", because reconciliation hands no checkout to anybody. | Doc, as directed. |
| **RE-4** platform note | Restated on `preserve_entry_checkout`: off unix `identities_reverify` is **always false**, so every preservation there settles `PreservationUnknown{AmbiguousCleanup}` with a downgraded locator and `Preserved` is unreachable — more protective, but a real claim-quality cost. | Doc. |
| **RE-5** §5.1 step 5 | Named as 2d's on `CheckoutPreservationV1` rather than left silently absent. | Doc. |
| **RE-1 / RE-2** | Obligation-table P2 row and the P4 "precise statement" corrected below. | Doc. |

**Not touched, per the adjudication:** disposition monotonicity across cell eviction (opus W3 → 2c2);
transient-refusal `Reserving` orphan (sol B-5 → stays the accepted 2b1 trade, recorded in §4.4);
Complete-persisted-today (sol B-6 → owner-accepted for V2, slice-5 remainder for V3);
blocking-cell liveness on shutdown (opus S1 → ledger); race-test determinism (sol S-1 → ledger);
trait-default composition invariant (sol S-2 → ledger).

## 2. Obligation table

| # | Binding item (source) | Test(s) | Status |
|---|---|---|---|
| P1 | durable `PreservationPrepared → Preserved` with the required claim | `a_live_checkout_settles_preserved_with_its_required_claim` | DONE |
| P1 | the edge order is prepared-then-terminal, no shortcut | `preservation_publishes_prepared_before_its_terminal_state` (fault-injected on rename 2) | DONE |
| P1 | no illegal edge is invented from a non-`LiveProtected` from-state | `preservation_refuses_a_from_state_with_no_legal_edge` | DONE |
| §5.7 row 5 | `claim_renamed_with_ambiguous_parent_sync_stays_protective` | same name (`custody_writer.rs`) | DONE |
| §5.7 row 12 | `preserved_claim_awaits_r2f2_with_no_provider_replay` | same name (`custody_writer.rs`) — byte-for-byte no-op — plus the provider half in `failure_cancel_and_ambiguity_never_call_provider_remove_reset_clean_prune` | DONE |
| P2 | `preservation_precedes_cancel_observed_at_every_enumerated_entry` — preflight | `preservation_precedes_cancel_observed_at_the_preflight_entry` | DONE |
| P2 | — cold prompt-open cancellation | `preservation_precedes_cancel_observed_at_the_cold_prompt_open_cancellation` | DONE |
| P2 | — cold drain cancellation | `preservation_precedes_cancel_observed_at_the_cold_drain_cancellation` | DONE |
| P2 | — rich-flush destroy, NO preceding cancel | `preservation_precedes_the_teardown_at_the_cold_rich_flush_failure_site` | DONE |
| P2 | — the R-11 fan-in entries (warm, SessionManager, both `Drop`s, retire) | `the_flight_side_barrier_preserves_before_the_inner_teardown` + `preservation_precedes_the_session_death_signal_at_every_backend_entry` (cancel/forget/release) | **CORRECTED (RE-1).** These entries reach the barrier's LOCATION — every one of them funnels through `run_cleanup_flight` — but **none of them ever ARMS it**, and by the RE-3 ruling none should: they have no workflow outcome, so they gate-retain and 2c2 disposes. The tests pin the flight-side barrier's placement for a session whose disposition was already raised; they do NOT show any of these callers preserving, because none does. See §4.2. |
| P2 | a normal exit must NOT preserve | `a_normal_node_exit_never_invokes_the_preservation_barrier`; `an_ordinary_teardown_leaves_a_live_checkout_live_and_still_undeletable` | DONE |
| P2 | `failure_cancel_and_ambiguity_never_call_provider_remove_reset_clean_prune` | same name (`backend.rs`) | DONE |
| P3 | preserve must not join an equal-strength reclaim flight | `a_preserve_request_never_joins_an_equal_strength_reclaim_flight` | DONE |
| P3 | the reverse order, and monotonic non-downgrade | `a_later_reclaim_cannot_downgrade_a_preserved_checkouts_disposition` | DONE |
| P4 | a refused/retained checkout must never project `Complete`/`released` | `a_retained_checkout_publishes_its_own_teardown_code_not_a_released_one` (all three outcomes in one test) | DONE for the teardown event; **remainder** for `NodeCleanupDispositionV1` and session-manager bookkeeping (§4.1) |
| P4 | the failed-configure retry contract survives (`Err` must not spin) | refusal still reports `Ok`; whole pre-existing suite incl. `failed_configure_cleanup_has_owned_retry_and_blocks_new_allocation` | HELD |
| P5 | ownership survives the initial refusal, bound rollback | `a_refused_bound_rollback_retains_its_owner_and_removes_exactly_once_later` | DONE |
| P5 | ownership survives the initial refusal, legacy rollback | `a_refused_legacy_rollback_retains_its_owner_and_removes_exactly_once_later` | DONE |
| P5 | remove protection later → exactly-once removal | both tests above (two releases, one removal) | DONE |
| P5 | the retention's scope is custody evidence, not any refusal | `an_inconclusive_probe_refusal_does_not_retain_the_entry` | DONE |
| P6 | a `Preserved` checkout is not handed to a new session | `a_preserved_checkout_is_never_reused_as_a_session_cwd`; `the_legacy_configure_entry_also_refuses_a_retained_checkout` | DONE (both entry points) |
| P6 | V2 reuse byte-identical | `a_legacy_ready_entry_is_still_reused_exactly_as_before` | DONE |
| P7 | swap source/common-dir → refuse to claim the replacement | `preservation_refuses_to_claim_a_swapped_source_and_settles_protective` | DONE |
| P7 | the reverification predicate itself discriminates both ways | `identity_reverification_passes_untouched_objects_and_fails_one_swap` (untouched / swapped / vanished) | DONE |
| P7 | protected-but-no-evidence refuses the claim and still refuses the deletion | `a_protected_entry_with_no_retained_identities_refuses_to_mint_a_claim` | DONE |
| small | `before_rename` injection for the replace path (2b1 opus S-8) | consumed, not re-added — `preservation_publishes_prepared_before_its_terminal_state` | DONE (pre-existing seam) |
| small | ambiguity-discard hardening at this slice's call sites (2b1 opus S-7) | one `From` conversion + `#[must_use]`; `claim_renamed_with_ambiguous_parent_sync_stays_protective` asserts `!is_terminal_preservation()` | DONE |
| small | V2 failure/cancel byte-identical | `the_preservation_barrier_is_a_no_op_for_a_legacy_checkout` + the unmodified 175-test base suite | DONE |
| 2b1 ledger | hold the refusing publication cell across probe→removal→**settlement** (sol SMELL-1, the half 2b2 left here) | the removal window now spans the settlement block; `a_cleanup_is_refused_while_a_writer_holds_the_checkout_publication_cell` (pre-existing) | DONE |
| RA | a stranded `PreservationPrepared` resumes to exactly one terminal state | `a_stranded_prepared_record_resumes_to_exactly_one_terminal_state`; `a_resume_reverifies_the_live_objects_and_never_trusts_the_stranded_claim` | DONE |
| RB | a failed reverification downgrades the claim's locator | swap test (extended) + `a_vanished_object_also_downgrades_the_claims_recovery_locator` | DONE |
| RC | every V3 non-success cold teardown raises `Preserve`; success stays `Reclaim` | `the_cold_exit_disposition_mapping_preserves_only_non_success_exits` + 3 family witnesses + the success control | DONE (13 converted + 2 computed sites) |
| RD | materialization evidence survives inner-configure failure and cancellation | `custody_evidence_survives_an_inner_configure_failure_after_materialization`; `custody_evidence_survives_a_cancelled_configure_after_materialization` | DONE (bounded form; residual window named in §4.7) |
| RE | labels, rulings, platform note, §5.1 step 5 | type split + doc comments; no behaviour beyond RE-6/6b | DONE |
| Non-goal | no `DeleteAuthorized` / `DeletionCapabilityV1` / `remove_v2` / post-loop mint; no claim exchange; no flock reclamation; no `workload_identity()`; no V2→V3 terminal-row cutover; no new transition-table edges | — | HELD (§4.5) |

### Mutation checks (all reverted; not checked-in artifacts)

| # | Mutation | Expected red | Observed |
|---|---|---|---|
| M1 | drop the disposition half of the join key (`same_disposition = true`) | `a_preserve_request_never_joins_an_equal_strength_reclaim_flight` | RED |
| M2 | move the flight barrier BELOW the inner teardown | `the_flight_side_barrier_preserves_before_the_inner_teardown` | RED |
| M3 | never retain a refused entry | both P5 rollback tests | 2 RED |
| M4 | collapse the teardown code back to the strength's own | `a_retained_checkout_publishes_its_own_teardown_code_not_a_released_one` | RED |
| M5 | skip claim-mint reverification (`verified = true`) | `preservation_refuses_to_claim_a_swapped_source_and_settles_protective` | RED |
| M6 | do not promote a preserved `Ready` entry to `Retained` | `a_preserved_checkout_is_never_reused_as_a_session_cwd` | RED |
| M7 | remove the executor barrier at the rich-flush site | `preservation_precedes_the_teardown_at_the_cold_rich_flush_failure_site` (only that one of the four) | RED, other 3 green |
| M8 | publish the terminal state without `PreservationPrepared` | `preservation_publishes_prepared_before_its_terminal_state` | RED |
| M9 | re-publish over a terminal record (row 12) | `preserved_claim_awaits_r2f2_with_no_provider_replay` | RED |
| M10 | **RA** re-publish `PreservationPrepared` on resume | `a_stranded_prepared_record_resumes_to_exactly_one_terminal_state` | RED |
| M11 | **RA** restore the old "no legal edge" refusal | both resume tests | 2 RED |
| M12 | **RB** pass the locator through unchanged | swap + vanish locator assertions | 2 RED |
| M13 | **RC** `ColdExitV1::Failure => None` (the pre-repair no-op) | the mapping table + all 3 family witnesses | 4 RED |
| M14 | **RD** upgrade custody only at the `Ready` publication | both RD tests | 2 RED |

**M2's first attempt did NOT discriminate, and the test surface was wrong, not the mutation.** The
caller-side witness (`preservation_precedes_the_session_death_signal_at_every_backend_entry`) stayed
green with the flight barrier moved below the teardown, because in that test
`preserve_checkout_v1` had already terminalized the record before the flight began. That is a real
gap in the evidence, not a quirk: it means the caller-side witness says nothing about the
flight-side placement. `the_flight_side_barrier_preserves_before_the_inner_teardown` was added for
exactly that, and M2 is red against it. Recorded rather than quietly fixed.

**Red-first honesty.** The two writer-side properties that came out red-first for a reason I did not
anticipate were 2a's identity-completeness rule (every preserving state requires observed `dev`/`ino`
on all four claim identities, which the shared `identities()` fixture violates) and the
`FakeProv` common-dir fidelity gap below. Everything else was written implementation-first and is
backed by the mutation evidence above rather than by a recorded red run.

**One test-double fidelity fix, called out because it changes what a double asserts.**
`FakeProv::add_under_custody` now CREATES the common dir it reports instead of merely naming it. A
real `git worktree add` reports a path that exists, and a double that does not makes every
preservation refuse for a reason production never has. No production code depends on it.

---

## 3. Gate outputs

Run in this worktree at `161e7ac0` plus the `cargo fmt` fix folded on top.

```
git diff --check                                          exit 0, no output
cargo fmt --all -- --check                                exit 0
cargo check --workspace                                   exit 0, zero warnings
cargo clippy --workspace --all-targets -- -D warnings     exit 0, zero warnings
cargo test -p bridge-core -p bridge-worktree -p bridge-coordinator \
           -p bridge-controller -p bridge-workflow -p a2a-bridge
      => 2614 passed; 0 failed; 11 ignored   (exit 0)      [after the repair round]
      (2605 passed before the repair round; +9 net new tests)
```

Base at `a9962e25` was 2577 passed / 11 ignored, so **+37 net new tests overall**. Per-package after
the repair round: `bridge-worktree` lib 175 → 203 (+28: 11 in `custody_writer`, 17 in `backend`);
`bridge-workflow` lib 124 → 133 (+9); `bridge-core` lib 478 → 481 (+3, pre-existing count drift from
the 2b2 fold, not this slice); `bridge-worktree::r2f1b_deletion_gate` 5, unchanged; `a2a-bridge` bin
1081, unchanged.

**Not run, and why:** the full workspace `cargo test --workspace` (the dispatch brief scopes this
slice to the six packages above and assigns the workspace suite to the orchestrator at fold),
`cargo build --release --bin a2a-bridge`, `cargo run -p a2a-bridge -- validate --repo-hygiene`,
`cargo deny`.

**Platform exclusions carried forward:** non-unix `dev`/`ino` (risk R-10) — `identities_reverify`
compares `directory_identity`, whose `dev`/`ino` are absent on non-unix, so on that platform the
predicate degrades to a canonical-path comparison and P7's substitution defence does not hold; the
completeness rule in `validate_object_identity` is likewise `#[cfg(unix)]`. Named here, untested.
Linux-arm execution of the custody primitives and real-NFS error-after-effect behaviour remain
carried from the 2b1 / PARKED-1 ledgers; darwin only. Nothing here ran against a network filesystem.

---

## 4. Parked findings, omissions, declared remainders

### 4.1 DECLARED REMAINDER — P4's `NodeCleanupDispositionV1` and session-manager halves

Shipped: the typed disposition end-to-end inside `bridge-worktree` (flight report, cell, map state)
and its projection onto the **worktree teardown event**, which is a real, production-reachable
observer surface.

Not shipped, with reasons:

1. **`NodeCleanupDispositionV1::Retained` was deliberately NOT added.** The enum is a frozen V2 wire
   contract (`NodeCleanupV1` inside `NodeTerminalV1`), and the brief's constraint is that V2 wire
   bytes stay unchanged. Adding an unused variant leaves existing bytes unchanged but creates a
   value an older binary cannot decode, for a producer that cannot exist yet: the executor maps
   ok/err to `Complete`/`Failed` in `cleanup_cold_session`, whose backend calls are
   `forget_session_observed` / `release_session_observed` — and the V3 route that would produce a
   retained disposition is production-unreachable (2b2 §2c). A dead variant on a frozen wire enum is
   the exact "dead wire contract" shape the 2b2 review flagged for `UnusedSettled`. **Owner: the
   slice that makes V3 production-reachable** (slice 5, per §5.3's ruling on the terminal-row
   cutover), which is also the slice that gains a producer.
2. **Session-manager owner bookkeeping is NOT wired.** The session manager reaches the backend only
   through `Arc<dyn AgentBackend>`'s `Result<(), BridgeError>` cleanup methods. Carrying a
   disposition there needs either a signature change on those methods (114 test doubles — risk R-5
   assigns that churn to slice 3) or a new query method with no production reader until slice 5.
   What IS available today: any session-manager caller using the *observed* variants sees
   `worktree.teardown.retained` / `.preserved`; the non-observed `release_session_checked` path has
   no channel at all.

Precise statement of what remains, restated per the review (RE-2): *only the DIAGNOSTICS transition
code distinguishes retained from released.* The durable node-terminal row still records
`NodeCleanupDispositionV1::Complete` for a retained checkout, because the executor maps a cleanup
`Ok` to `Complete` and this slice does not change that mapping — so a reader of the persisted
terminal cannot tell the two apart, and that boundary holds until slice 5. What a caller CAN
distinguish today: an observed cleanup sees `worktree.teardown.retained` / `.preserved`; anything
inside `bridge-worktree` sees the typed `CheckoutCleanupDispositionV1`. A caller using a
non-observed `AgentBackend` cleanup method has no channel at all.

### 4.2 The warm and SessionManager entries reach the barrier's LOCATION but never ARM it (corrected, RE-1)

**The first version of this section overstated the coverage, and opus was right.** These entries are
covered for *placement* — they all funnel through `run_cleanup_flight`, where the barrier sits ahead
of the inner teardown — but **no warm, SessionManager, reaper, `Drop`, or retire caller ever calls
`preserve_checkout_v1`**, so none of them arms preservation, and by the RE-3 ruling recorded in code
none of them should: they have no workflow outcome, so an unconditional manager-side `Preserve`
would terminalize a healthy warm session that merely went idle. What they get is the gate: retained,
reported `Retained`, disposed by 2c2's post-loop mint. The tests below pin the barrier's placement
for a session whose disposition was raised by someone else; read them as placement evidence, not as
evidence that these callers preserve.

The path-collapse argument for the *placement* claim, unchanged:

`cleanup_warm_turn` takes a `Box<dyn NodeTurnCleanup>` and no backend, so there is no place in it to
call the barrier; the trait has zero production impls, and the warm owner reaches the backend
through `SessionManager`. Every `SessionManager` entry — `ExpiryClaim::{cleanup, into_flight, Drop}`
via `start_flight`, and the eleven direct `release_session` sites — calls
`release_session_checked`/`forget_session_checked`, which the flight-side barrier covers and which
`the_flight_side_barrier_preserves_before_the_inner_teardown` pins. 2b1 already proved the real
`reap_idle → claim.cleanup() → start_flight → release_session_checked` chain arrives at that method
(`tests/r2f1b_deletion_gate.rs::idle_reaping_cannot_delete_a_custody_protected_checkout`), and that
test is unchanged and green. No new integration test was added, and that is a judgement call: the
collapse is the same one 2b1 documented, and reproducing a V3 materialization inside the integration
crate (which compiles `bridge-worktree` without `cfg(test)`) would cost ~120 lines to re-prove a
chain already pinned. **Flagging it as a coverage choice, not asserting it is equivalent to an
end-to-end run.**

### 4.3 SIZE — at the checkpoint, disclosed

Measured against `main`, **after the repair round**: **1,370 non-test lines added** (a large share
doc comment, per house style) and **1,714 test lines**, **3,084 total** across 4 files. Before the
repair round: 1,112 / 1,231 / 2,343.

The brief's checkpoint was "stop and report if NON-TEST exceeds ~1,100 or the total heads past
~2,800". The first crossing happened during the original slice, at 1,112 — 1% over, inside P4's
report plumbing, which cannot be half-landed (the flight report type change touches every waiter);
everything after that measurement was test-only. The repair round then added ~740 lines, 65% of it
test surface, and a declared repair round is not a place to stop for size — RC alone converts 15
call sites and RD needed a new inner double to make two arms reachable at all. Both thresholds are
now exceeded. Reported rather than absorbed.

### 4.7 RD's residual window, named rather than closed

The reservation-side custody upgrade lands immediately after `materialize_under_custody` returns and
before the next await, but the materializing `spawn_blocking` can complete and the configure future
can be dropped before the map write executes. The record is durable by then and the disk gate
contains the consequence (nothing deletes it), so the residual cost is the same evidence loss over a
strictly smaller window. Closing it needs a claimed, non-cancellable materialization flight — that
is §2.5's preparation flight, whose runner is **slice 3's**, and the review LEDGERED it there rather
than having it improvised here. Stated in the code comment as well as here.

### 4.4 Deliberate omissions and accepted costs

* **A failed reverification loses the trigger reason.** The record's reason becomes
  `AmbiguousCleanup`, so an R2f2 consumer learns *that* the identities did not verify but not
  whether the node failed or was cancelled. Adding a seventh `PreservationReasonV1` value would
  change 2a's frozen wire contract, which this slice must not do. The trigger is in the log line and
  in the in-process outcome.
* **A `LiveProtected` V3 checkout torn down by a context-free entry gets no claim.** It is retained
  (the gate refuses) and reported `Retained`, but no R2f2 artifact is written, because §5.1 reserves
  that decision for the workflow-level disposition. If 2c2's post-loop mint slips, those checkouts
  leak with no claim — the same deliberate leak risk R-3 already accepted, one notch weaker.
* **`RecoveryLocatorV1::RegisteredWorktree` is materialization-time evidence, not a re-probe.** Named
  under P7. A registration re-probe needs a new `WorktreeProvider` operation with a refusing default
  across all ten impls; out of scope here.
* **The barrier's default is a no-op, enforced by composition rather than by construction.** Verified
  that `WorktreeBackend` is the outermost production decorator today. A future wrapper placed outside
  it would silently skip preservation with no test failure. This is the same call-site-property
  weakness 2b2's §2c recorded for `r2f1b: None`, and the same remedy would apply.
* **A transient-refusal (`ProbeInconclusive` / `CellContended`) rollback of a `Reserving` entry
  still drops its map owner** — the retention is scoped to custody-positive refusals only. Reviewed
  and ruled CORRECT in both directions: this is 2b1's accepted self-healing V2 trade (the legacy
  `.meta.json` is retained, so the run-end guard or the next boot sweep reclaims it), and retaining
  on an unknown would wedge a session id after one transient `EACCES`. Recorded here as an accepted
  residual rather than left implicit.
* **No `#[cfg(not(unix))]` behaviour was exercised**; darwin only — and per RE-4 the non-unix
  behaviour is not merely untested but *different*: `identities_reverify` is always false there, so
  `Preserved` is unreachable and every preservation settles `PreservationUnknown{AmbiguousCleanup}`.

### 4.5 Non-goals held

No `DeleteAuthorized` CAS, no `DeletionCapabilityV1`, no `remove_v2`, no post-loop mint (2c2). No
claim exchange / `RecoveredLive` production (2d). No `.custody-locks` flock reclamation (owner
decision OPEN — untouched). No `workload_identity()` wiring (slice 4). No V2→V3 terminal-row cutover
(slice 5). **No new transition-table edges** — `LEGAL_CUSTODY_TRANSITIONS_V1` is byte-identical, and
`preservation_refuses_a_from_state_with_no_legal_edge` pins that the driver will not publish outside
it. The 13 legacy `configure_session` tests are untouched.

### 4.6 No parked defects

Nothing in merged code was found to be wrong during this slice. Two shortcomings in *this slice's own
first draft* are recorded above rather than hidden: the non-discriminating first version of the M2
witness (§2), and the `FakeProv` common-dir fidelity gap.

---

## 5. §2c SELF-PASS (adversarial, NOT INDEPENDENT)

**Claim under test:** *no failure, cancel, or ambiguity path in this slice can reach provider
removal, reset, clean, or prune for a custody-discriminated checkout — preservation only.*

**Refutation condition:** any new preservation, barrier, or disposition path that can invoke a
destructive provider or git operation on a protected entry.

**Search scope.** Three independent sweeps, because any one of them alone would be an argument from
absence:

1. **Token sweep over the added NON-TEST lines** of all four changed files (`git diff main`, hunk
   offsets tracked so test-module lines are excluded, comments stripped before matching), for
   `remove_file|remove_dir|.remove(|prune|reset|clean|checkout --|verify_then_remove|unlink|kill|
   SIGKILL`. **Zero hits.** Every destructive token in the diff is `std::fs::remove_dir_all(&tmp)`
   inside `#[cfg(test)]` fixture teardown.
2. **Call-graph read of each new path**, individually rather than inferred from (1):
   `preserve_checkout_v1` (map read/write only) → `preserve_entry_checkout` (`spawn_blocking`) →
   `WorktreeCustodianV1::enter` (two file locks + `PinnedDirectoryV1::open`) →
   `preserve_after_cancel` → `current_state_kind` (`child_entry_exists` + `read_custody_record_in`,
   both read-only) and `replace_preserving_state` → `stage_and_settle` → `create_new` + fsync +
   `publish_new_regular_child` / `replace_regular_child` + parent sync, then `settle_residue` /
   `quarantine_residue`, **both of which unlink nothing in any arm** (2b2 repair R3, pinned by
   `a_durable_publication_never_unlinks_the_staging_pathname`). `retain_refused_entry`,
   `raise_checkout_disposition`, the configure `Retained` arms and
   `preserve_checkout_before_signal` perform no filesystem work at all.
3. **The gate is unchanged and still gates.** `checkout_removal_refusal` and
   `CheckoutRemovalWindowV1::enter` are byte-identical to 2b2's; the barrier runs strictly *before*
   the window is entered, and `WtEntry.custody` is never written back to `Legacy` anywhere in the
   diff. For a `Discriminated` entry the gate's first arm refuses unconditionally, so no sequence of
   new calls can make a custody-discriminated checkout deletable.

**Alternative mechanism that would produce the same "zero hits" output** (evidence-admissibility
check): a destructive call reached through an *existing* symbol whose name does not match the token
set — e.g. `provider.remove(...)` written as a method on a renamed binding, or the removal block
being reached with a weakened refusal. Sweep (3) is what discriminates that, plus
`failure_cancel_and_ambiguity_never_call_provider_remove_reset_clean_prune`, which counts actual
provider `remove` invocations through a double rather than reading source.

**Re-run after the repair round.** Sweep (1) re-executed over the full post-repair diff: still
**zero** destructive-token hits in added non-test code. The repair round adds no filesystem
operation anywhere — RA and RB change which state and locator a rename publishes, RC and RD only
raise a disposition and write map entries, RE is documentation and a type split. Sweep (3) is
unaffected: `checkout_removal_refusal` and `CheckoutRemovalWindowV1` are still byte-identical to
2b2's, and `WtEntry.custody` is still never written back to `Legacy` — RD writes it *to*
`Protected`, which is strictly more protective.

**Verdict: SURVIVED — with two corrections left visible.**

**Correction 1 (scope of "custody-discriminated").** The claim holds exactly for entries whose
in-memory discriminator is `Protected`, and for any entry with a custody record present on disk. It
does **not** say that a `Retained` entry can never be removed: it can, deliberately, once its custody
record is gone (R2f2 disposition) and the entry's discriminator is `Legacy` — that is precisely the
"exactly-once removal after protection lifts" behaviour P5 was required to deliver, and
`a_refused_bound_rollback_retains_its_owner_and_removes_exactly_once_later` asserts it. A reader who
took the claim to mean "nothing this slice touches is ever deletable" would be wrong. The accurate
statement: *while custody evidence exists — the discriminator, the record on disk, or both — no path
this slice adds can remove, reset, clean, or prune the checkout; when all custody evidence is gone,
the ordinary V2 removal proceeds through the unchanged gate.*

**Correction 2 (what "ambiguity" covers).** The ambiguity arm the claim names is tested at the
writer (`claim_renamed_with_ambiguous_parent_sync_stays_protective`, which asserts
`!is_terminal_preservation()` and a protective on-disk state) and structurally at the backend (an
`Ambiguous` outcome maps to `CheckoutRetentionV1::PreservationUnknown`, and the gate below is
unchanged). It is **not** driven end-to-end through the backend with an injected parent-sync fault —
`failure_cancel_and_ambiguity_never_call_provider_remove_reset_clean_prune` reaches its third case
through the already-terminal arm, not through a live ambiguous publication. The mechanism is the
same code path either way, so the claim survives, but the backend-level ambiguity evidence is
weaker than the failure and cancel arms and should be read that way. The repair round strengthened
the *writer-side* ambiguity evidence rather than the backend-side: RA's
`a_stranded_prepared_record_resumes_to_exactly_one_terminal_state` drives a record that is stranded
by a real injected rename fault, and RE-6 stopped the backend mislabelling that state as terminal.
The backend-level gap stands as stated.

**Also checked and cleared, for the record:** the barrier acquires the publication cell with the
*blocking* acquirer while `cell.state` (the per-session single-flight async mutex) is held. That
cannot deadlock: the only other holders of a checkout's publication cell are the V3 writer (which
never takes `cell.state`) and refusing acquirers (the gate and both sweep arms), which never wait.
The barrier releases both cells before the gate's own refusing acquisition, so the two never
interleave within one flight.
