# R2f1b slice 2b2 — V3 routing, the writer, and creation ordering — implementer handoff

Date: 2026-08-09. Branch `feat/r2f1b-2b2-routing-writer`, base local `main` @ `8255cf5f` (2b1 +
the PARKED-1 publication-classification fix). Four commits: `4abfa45b` (routing + admission
refusal), `5f8affc8` (writer + add prohibition + gate cell), `8e790f89` (crash matrix + real-git
add prohibition evidence), `9549f2ac` (sweep coexistence + settlement + storage report).

---

## 1. What shipped, per S1–S7

### S1 — Routing: admission → executor → `BoundSessionSpecV1`

| Symbol | File |
|---|---|
| `BoundWorktreeCustodyV1` | `bridge-core/src/execution_policy.rs` |
| `CustodyPlanBindingRefusalV1` | `bridge-core/src/execution_policy.rs` |
| `select_custody_plan_v1` | `bridge-core/src/execution_policy.rs` |
| `BoundProviderEffectV1::{custody, bind_custody_plan}` | `bridge-core/src/execution_policy.rs` |
| `BoundSessionSpecV1::custody` | `bridge-core/src/execution_policy.rs` |
| `WorkflowDiagnosticContext::with_frozen_r2f1b_contract` | `bridge-workflow/src/executor.rs` |
| `WorkflowExecutor::bind_frozen_entry` (V3 arm) | `bridge-workflow/src/executor.rs` |

Selection is EXACT — `FrozenCheckoutEffectV1::Worktree.checkout_digest ==
FrozenWorktreeCustodyPlanV1.checkout_fingerprint` — and `bind_custody_plan` re-verifies digest,
target, and attempt lineage at construction, so an unverified pairing is unrepresentable. A
worktree checkout with no matching plan REFUSES (`NoPlanForCheckout`) rather than degrading to V2;
a direct checkout selects nothing and does not refuse. Plan coverage over the whole candidate
matrix is checked once at admission, not per node, so a partially-materialized graph cannot be
refused mid-run.

### Design note 1 — where the propagated plan lives

**Decision: on `BoundProviderEffectV1` (private field + accessor), not as a new field on
`BoundSessionSpecV1` and not as a wrapper.** Full rationale is on the field itself. In short:

1. `BoundSessionSpecV1` has public fields and is built by exhaustive struct literal outside
   bridge-core (`bridge-container/src/lib.rs`, twice). A new public field breaks those literals; a
   new private field makes them impossible. `BoundProviderEffectV1`'s fields are already private
   and it is constructed in exactly one function (`freeze_provider_attempt_v1`), so a binding
   added there reaches every consumer that already holds the `Arc` with **zero** call-site ripple
   and V2 routes byte-identically.
2. The matching key is already there: `self.frozen.checkout`. Putting the plan beside the value it
   is matched against lets the constructor re-verify the match, so the invariant is structural
   rather than a convention selection has to remember.
3. One binding per provider effect is the true cardinality — a session spec is rebuilt per
   configure call from a shared `Arc`, so a spec-resident plan would need re-attaching (and
   re-verifying, or not) every time.

`BoundSessionSpecV1::custody()` is a delegation, so the discriminator the backend branches on
cannot drift from the effect that was actually verified.

### S2 — The V3 writer and the creation-ordering inversion

New module `bridge-worktree/src/custody_writer.rs`:
`WorktreeCustodianV1::{enter, publish_protection_prepared, replace_materializing,
replace_live_protected, replace_preservation_unknown}`, `MaterializedIdentitiesV1`,
`planned_identity`, `observed_identity`, `is_staged_custody_residue`, `CustodyWriteRefusalV1`.
Backend fork: `WorktreeBackend::{materialize_checkout, materialize_under_custody}`
(`bridge-worktree/src/backend.rs`), plus `custody_write_error` / `wall_clock_ms`.

Order (§2.5): publication cell → custody cell → pin root by descriptor → stage + `fsync` →
**no-replace** publish `ProtectionPrepared` → parent sync → replace `Materializing` → parent sync
→ `git worktree add` → capture four identities by descriptor → replace `LiveProtected`. Both cells
are held across the add, deliberately: the record must stay this custodian's for the whole window
in which the checkout is half-made, and the add takes no custody cell so it cannot deadlock. The
whole blocking sequence runs under `spawn_blocking` per `custody_lock.rs`'s acquisition contract.

`WtCustodyV1::Protected` is now set by the writer at the `WtState::Ready` construction site
(previously `Legacy` unconditionally). The V3 path never calls `write_sidecar`; the V2 path is
untouched, in its original order, inside the same function.

Identity capture is by descriptor (R-8's accepted disposition): three of the claim's four object
identities do not exist in `FrozenWorktreeCustodyPlanV1` at all.
`RecoveryLocatorV1::RegistrationUnproven` is produced by `host_git::classify_custody_add_failure`
from the registration probe's `Err` arm — 2a's docstring'd obligation; the variant was unreachable
until now.

### S3 — Custody-aware add and the `cleanup_failed_add` prohibition (same PR)

`WorktreeProvider::add_under_custody` with a REFUSING default, plus `CustodyAddOutcomeV1`,
`CustodyAddFailureV1`, `CustodyAddTargetV1` (`bridge-worktree/src/provider.rs`).
`HostGitWorktree::add_under_custody` + `classify_custody_add_failure`
(`bridge-worktree/src/host_git.rs`). `add` is UNCHANGED — it serves V2.

`Err` (the default) means "this provider has no custody-aware add"; `Ok(Failed(..))` means "the add
ran and did not succeed". Conflating them would let an unmodified provider look like one whose add
merely failed, and the writer would publish a record for a checkout no provider is protecting.

**Ten impls enumerated** (the brief's nine plus `tests/r2f1b_deletion_gate.rs`'s `FakeProvider`,
added by 2b1), each with an explicit decision comment: `FakeProv` (V3-capable happy path),
`NonGitProv`, `SidecarWriteFailProv`, `PartialAddFailProv` (the partial-add double),
`BlockingRemoveProv`, `BlockingProv`, `BlockingProbeProv`, `workflow_planner::ProbeProvider`
(panics — reaching it is a routing bug), `r2f1b_deletion_gate::FakeProvider`, `HostGitWorktree`.

### Design note 2 — how the custody-aware add threads identities back

**Decision: through the RETURN TYPE, not an out-param record.** Two reasons, on
`CustodyAddOutcomeV1`:

1. The provider does not know the identities the claim needs. Three of the four
   `WorktreeObjectIdentityV1`s plus the target's own `dev`/`ino` are captured by descriptor by the
   *writer*, under the custody lock, after the add returns. A provider handed a record to fill
   would either re-open those objects itself (a second, unsynchronised identity capture) or leave
   them blank — and a partially-filled out-param is indistinguishable by type from a full one.
2. What the provider uniquely knows is exactly what is returned: the common dir, and the two probe
   answers (`target`, `recovery_locator`) only the git-level operation can produce. Returning
   precisely that keeps the trait honest — a test double cannot accidentally satisfy the identity
   contract by leaving an out-param untouched.

### S4 — `AutomaticR2f1b` refusal at the production admission boundary

`admit_r2f1b_contract_v1` (`bridge-workflow/src/admission.rs`), called from
`WorkflowAdmissionV1::freeze` AND `WorkflowDiagnosticContext::with_frozen_r2f1b_contract` — one
rule, both entrances, because the executor entry is `pub` and a rule enforced at one of two
entrances is not enforced. **Not** in `FrozenR2f1bContractV1::validate`: offline construction,
encoding, decoding and workload identity of an automatic contract stay legal (A3/A5). The rule also
refuses a successor attempt identity — §5.8's claim exchange is 2d/slice 5, and a successor here
would route a claim-less V3 write over a predecessor's live checkout.

`WorkflowAdmissionRequestV1.r2f1b` / `AdmittedWorkflowRunV1.r2f1b` are explicit `Option` fields
rather than defaulted, so the four production `r2f1b: None` sites stay greppable.

### S5 — Durable run-end settlement

`WorktreeRunEndGuard::{new, settle, is_settled, run_end_pass}` (`bridge-worktree/src/sweep.rs`)
and `settle_worktree_run_end` (`bin/a2a-bridge/src/main.rs`), wired at the three run-end-guard
sites (`implement`, `implement --resume`, `run-workflow`). Settled → `Drop` is a no-op; unsettled +
clean → the legacy reclaim as before (R9 leaves the clean path destructive); unsettled + unwinding
→ defer, and record that settlement did NOT occur rather than logging as though it had.

**Named exclusion**: the `mcp` and `serve` sweep entry points install a boot sweep but no run-end
guard at all — they are long-running servers with no run end to settle. Five sweep entry points,
three settleable.

### S6 — Sweep and storage-report obligations

* Coexistence guard in `sweep::remove_worktree_if_safe` — presence-keyed
  (`probe_custody_record_presence`), so a damaged record still protects. Covers BOTH arms including
  the run-end guard's clean-drop arm.
* Per-guard discrimination for `sidecar_file_matches` and `worktree_under_root`
  (2a carried item) — mutation-checked in both directions, see §3.
* `storage_report::{WorktreeRecordKindV1, custody_record_holder_state}` and the
  `.custody-locks` arm in `scan_worktree_root` — closes risk R-4 and the 2b1 opus S-10 item.

### S6b — Writer-side residue and fault counting

Residue policy is stated on `custody_writer`'s module docs and implemented in
`stage_and_settle` / `settle_residue` / `quarantine_residue`: nonce-named staging
(`<target>.custody.v1.json.staging-<32 hex>`) that matches NEITHER sweep pattern; unlinked only on
a provably-`Durable` rename; **quarantined** on every unproven arm and on a true `Err` (§5.7 row 2).
The nonce is what makes a retry converge rather than collide with its own residue. Fault counting
(one countdown shared by publish and replace) is pinned by
`the_publication_fault_countdown_counts_publishes_and_replaces_together`.

### S7 — Gate-side lock wiring

`custody_lock::{custody_publication_lock_id, try_acquire_publication_lock_in,
acquire_publication_lock_blocking_in, PublicationLockGuardV1}` and
`backend::CheckoutRemovalWindowV1` + `CheckoutRemovalRefusalV1::CellContended`.

The gate cannot name the CUSTODY cell — the custody id lives inside the record, and the gate's
discipline is presence-not-content, so it must never depend on decoding one. The only key both
sides certainly share is the canonical target path, so a second, strictly OUTER cell is keyed on
`sha256(canonical target path)`. Total order is **publication cell → custody cell**; the writer
takes both in that order, every deletion-side caller takes the publication cell only, with the
refusing acquirer.

**This supersedes the 2b1 dual-review ledger's assignment of "hold the refusing custody lock across
probe→removal→settlement" to 2c1 (sol SMELL-1).** The writer landing here is what made the
probe→removal half due now. What remains 2c1's is holding it across *settlement*, which needs the
typed retained/refused disposition 2c1 mints.

---

## 2. Obligation table

| # | Binding item (source) | Test(s) | Status |
|---|---|---|---|
| S3-a | custody-aware add lands in the SAME PR as the writer (2b1 ledger) | — (structural; one commit series, one branch) | DONE |
| S3-b | refusing default, enumerated across every impl (R-6) | `a_provider_without_a_custody_aware_add_refuses_before_any_checkout_exists` | DONE (10 impls) |
| S3-c | `add_failure_after_target_creation_never_removes_target` | same name (`backend.rs`) | DONE |
| S3-d | `add_failure_before_any_target_settles_unused_marker_only` | same name (`backend.rs`) | DONE **with disclosed deviation** — see §4.1 |
| S3-e | `partial_add_publishes_preservation_unknown_materialization_inflight` | same name (`backend.rs`) | DONE |
| S3-f | the routine refused-rollback → configure-retry → surviving-dir sequence (2b1 ledger) | `custody_add_failing_on_a_surviving_directory_never_removes_it` + V2 control `the_legacy_add_still_removes_the_same_directory` (real git) | DONE |
| S3-g | `registration_absent` `Err` → `RecoveryLocatorV1::RegistrationUnproven` (2a) | `an_unprovable_registration_maps_to_registration_unproven`; `preservation_unknown_carries_its_required_claim_and_the_probed_locator` | DONE |
| S6-1 | `a_checkout_carrying_both_records_is_reclaimed_by_neither_sweep_arm`, BOTH arms incl. run-end clean drop | same name (`sweep.rs`), parameterised over boot + clean-drop | DONE |
| S6-2 | `old_binary_sweep_cannot_select_a_v3_checkout` | same name (`backend.rs`) | DONE |
| S6-2 | `v3_path_writes_no_legacy_meta_json` | same name (`backend.rs`) + in-flight assertion in the ordering test | DONE |
| S6-3 | sweep redundant-guard coverage: a SINGLE-guard regression goes red (2a carried) | `sidecar_sibling_match_alone_stops_a_forged_in_root_sidecar`; `under_root_check_alone_stops_a_sibling_sidecar_pointing_outside_the_root` | DONE, mutation-checked both ways |
| S6-4 | V3 record as a second suffix: Evidence, sibling association, holder per custody state, V2 unchanged | `worktree_root_reports_a_v3_custody_record_as_evidence_binding_its_sibling`; `a_preserved_custody_record_still_holds_its_checkout`; `a_v2_only_worktree_root_reports_exactly_what_it_did_before` | DONE |
| S6-4 | classify `<root>/.custody-locks` (2b1 opus S-10) | `the_custody_lock_directory_is_classified_and_never_unclassified` | DONE |
| S6b-1 | staged-source residue policy defined + tested (PARKED-1 opus S-9) | `a_durable_publication_never_unlinks_the_staging_pathname`; `an_unverified_rename_never_unlinks_what_occupies_the_staging_name`; `a_foreign_record_refuses_the_first_publication_and_the_temp_is_quarantined` (the one arm that really keeps residue); `staged_residue_recognition_requires_exactly_thirty_two_lowercase_hex`; `every_minted_staging_name_satisfies_the_exact_predicate`; `staged_residue_matches_neither_sweep_pattern_and_never_collides_with_itself` | **REPAIRED (R3)** — the shipped policy had two pathname unlinks and NO test of its headline rule; the two tests previously cited here asserted something else and are renamed |
| S6b-2 | crash-matrix tests count every rename across publish+replace (PARKED-1 opus S-6) | `the_publication_fault_countdown_counts_publishes_and_replaces_together` | DONE |
| S7 | refusing acquirer around the gate's probe→removal window; writer-vs-delete raced both orders | `a_cleanup_is_refused_while_a_writer_holds_the_checkout_publication_cell` (both orders in one test); `both_cells_are_held_for_the_custodians_lifetime_and_released_on_drop` | DONE; reassignment from 2c1 recorded above |
| S4 | `automatic_r2f1b_refused_at_production_admission` / `manual_only_r2f1a_admitted` / `offline_automatic_contract_construction_still_legal` | same names (`r2f1a_admission.rs`) | DONE (+ 3 extra: coverage, successor lineage, executor-boundary twin) |
| §5.7 row 1 | before `ProtectionPrepared` publication → no worktree/provider/process effect | `custody_record_is_parent_synced_before_any_git_worktree_add` (the ordering witness taken INSIDE the provider) + `a_provider_without_custody_support_publishes_no_record_at_all` | **mapping CORRECTED (R9)** — the foreign-record test covers row 2, not row 1 |
| §5.7 row 2 | temp written, before rename → final absent, temp quarantined | same test (residue assertion) | DONE |
| §5.7 row 3 | prepared synced, before add → marker excludes sweeps | `protection_prepared_is_published_and_readable_before_any_provider_effect` | DONE |
| §5.7 row 4 | during/after partial add → preservation unknown, never delete | `partial_add_publishes_preservation_unknown_materialization_inflight` | DONE |
| §2.5 | record parent-synced BEFORE any `git worktree add` | `custody_record_is_parent_synced_before_any_git_worktree_add` | DONE, mutation-checked |
| Non-goal | V2 byte-identical; the 13 legacy `configure_session` tests untouched | whole `bridge-worktree` suite (158 lib tests, all pre-existing ones unmodified) | HELD |

---

## 2b. Repair round (dual-lens review: opus REVISE 4W+12S, sol REJECT 5B+2W)

One declared round, all items closed-enumerable. Per-item outcome:

| Item | Fix | Red-first evidence |
|---|---|---|
| **R1** routing handoff (sol B-1 / opus S-6) | `WorkflowDiagnosticContext::with_admitted_workflow_run` — one authority-consuming constructor that binds spec THEN the optional contract, DESTRUCTURING `AdmittedWorkflowRunV1` so a future field cannot be dropped silently. Wired at both production consumers (`main.rs` run-workflow, `detached.rs`). | `an_admitted_contract_survives_the_production_authority_binder` (real `WorkflowAdmissionV1::freeze` → real binder → backend) + V2 negative `an_admission_with_no_contract_still_routes_v2_through_the_same_binder`. Mutation (drop the contract, i.e. the shipped code): RED. |
| **R2** sweep-side cell (sol B-2) | `remove_worktree_if_safe` now: sibling+containment guards FIRST (also fixes opus S-8's ordering), THEN the path-keyed refusing publication cell held across probe + every removal op; contention/unavailability skip without deleting. | `a_writer_holding_the_publication_cell_stops_the_boot_sweep_and_releases_it`; `..._stops_run_end_settlement`; reverse order `a_writer_waits_when_a_reclaim_already_holds_the_publication_cell`; `a_forged_sidecar_never_touches_its_named_path_before_the_guards_pass`. Mutation (no cell): 2 RED. |
| **R3** residue identity + honesty (sol B-3 / opus W-4a,S-4,S-10) | BOTH pathname unlinks removed — `remove_file` addresses a name, not our descriptor. Module doc rewritten: no arm unlinks; residue genuinely survives only on a true `Err` and on `RenameOutcomeUnverified` with a foreign object at the source name; reclamation owner stated truthfully as **storage report + owner disposition**, NOT the boot sweep (which ignores staging names by design). Recognition tightened to exactly 32 lowercase hex. | `a_durable_publication_never_unlinks_the_staging_pathname` (mutation: RED); `an_unverified_rename_never_unlinks_what_occupies_the_staging_name`; `staged_residue_recognition_requires_exactly_thirty_two_lowercase_hex` (short/long/uppercase/non-hex/no-nonce/no-stem negatives); `every_minted_staging_name_satisfies_the_exact_predicate`. Two mis-named tests renamed with the reason in their docstrings. |
| **R4** false `Materializing` (sol B-4 / opus S-3) | New side-effect-free `WorktreeProvider::supports_custody_add` preflight BEFORE any record effect; a post-preparation runtime `Err` is normalized into the classified failure and SETTLED to `PreservationUnknown{materialization_inflight}` with `RegistrationUnproven`. | `a_provider_without_custody_support_publishes_no_record_at_all` (mutation: RED); `a_runtime_add_error_settles_preservation_unknown_instead_of_leaving_materializing`. |
| **R5** settlement epilogue (sol B-5 / opus W-2) | `implement`'s whole `decide` match wrapped (Abort / NoCommitClean / NoCommitDirty were bypassing it); run-workflow's output-write-failure early return folded into the epilogue. Log field fixed to `settled = self.is_settled()`. A non-panicking `Drop` now MARKS itself settled — a clean drop is a handled exit — so "unsettled" means exactly "panicked or unhandled". | `every_handled_outcome_settles_exactly_once_and_reports_it` (6-arm branch table); `a_clean_drop_of_an_unsettled_guard_counts_as_a_handled_settlement`; existing `an_abrupt_drop_is_protective_and_does_not_claim_settlement` still green. |
| **R6** gate cell vanished root (opus W-1) | `root.try_exists()` precheck moved BEFORE the cell attempt. The old order reached `create_dir_all(<root>/.custody-locks)` from a TEARDOWN path, re-creating a vanished `[worktrees].root`, and made the documented `Ok(None)` arm unreachable because `create_dir_all` prevented the error it keyed on. | Covered by the existing gate suite; the arm is now reachable by construction. |
| **R7** claim identity honesty (opus W-3) | Unobserved common dir now records the plan-derived `<source>/.git`, not the source repo — the previous value asserted the source directory IS the common dir. | Existing `partial_add_publishes_preservation_unknown_materialization_inflight` covers the arm. |
| **R8a** storage-report coexistence (sol W-6) | Filename sort order let a FREE legacy sidecar overwrite a HELD V3 record (`.custody.v1.json` sorts first). Now merged through the module's EXISTING `merge_holder` lattice (Held dominates, Unknown beats Free) rather than a second hand-rolled rule. | `a_live_custody_record_holds_its_checkout_even_beside_a_free_legacy_sidecar` (mutation: RED). |
| **R8b** lock order (opus S-1) | `custody_lock.rs`'s declared global order re-declared to include the publication cell's true position, plus an explicit note on the gate's file-lock-inside-`cell.state` nesting and why it cannot cycle. | Doc. |
| **R8c** bind doc (opus S-5) | `bind_custody_plan`'s doc corrected: it verifies digest, target, and the ordinal-0 rule only — NOT `parent_attempt_id` or successor lineage, which `validate_successor` owns and which admission refuses outright for slice 2. | Doc. |
| **R8d** cell coverage (opus S-2) | Five publication-cell unit tests: distinct-path non-contention, pure-function key, blocking acquirer + contention callback, stable path after release, no aliasing with custody cells. | New tests. |
| **R9** naming/handoff (opus W-4) | Test renamed per the ruling; §5.7 row-1 mapping corrected below; §2c count corrected four→three; obligation rows updated. | Doc. |

**Pushback: none.** Every item reproduced against the source. Two were sharper than the summary
suggested and I widened the fix accordingly: R3's durable-arm unlink is not merely unproven-identity
but *only ever* destructive (it is a no-op unless a foreign file occupies the name), and R5's log
field disagreed with `is_settled()` in opposite directions at the same instant, which is why the
clean-drop path now marks itself settled rather than only fixing the field.

**Surfaced by the repairs, not in either review:**

1. `merge_holder` already existed in `storage_report.rs` with a BETTER lattice than the one I was
   about to add (Unknown beats Free — "never let one runtime's 'nobody' mask another's 'cannot
   tell'"). Reused it; my draft would have regressed that rule for these two maps.
2. R4's refusing-default test initially asserted zero provider removals and failed: the ordinary
   rollback *does* call `provider.remove`, correctly — with no record published there is genuinely
   nothing under custody, the gate authorizes on that evidence, and the target never existed.
   Asserting zero would have been asserting that a refused configure skips its rollback. Assertion
   corrected with the reasoning recorded in the test.
3. `supports_custody_add` introduces a two-method coupling (a provider could claim support and not
   implement the add). Both default to the refusing pair so an untouched impl is consistent by
   construction; the obligation is documented on the predicate. Ledger item if a stronger tie is
   wanted.

## 3. Gate outputs

Run in the worktree at `9549f2ac` + the format/lint fixes folded into it.

```
git diff --check                                            clean
cargo fmt --all -- --check                                  clean
cargo check --workspace                                     clean
cargo clippy --workspace --all-targets -- -D warnings       clean
cargo test -p bridge-core -p bridge-worktree -p bridge-coordinator \
           -p bridge-controller -p bridge-workflow -p a2a-bridge
      => 2577 passed; 0 failed; 11 ignored   (exit 0)      [after the repair round]
      (2557 passed before the repair round; +20 net new tests)
```

Per-package highlights after the repair round: `bridge-worktree` lib 175 passed (131 at base);
`bridge-worktree::r2f1b_deletion_gate` 5 passed; `bridge-core` lib 1080 passed; `a2a-bridge` bin
incl. 53 storage-report tests; `bridge-workflow` `r2f1a_admission` 10 passed,
`r2f1a_bound_executor` 19 passed.

**Not run, and why:** the full workspace `cargo test --workspace` (the orchestrator runs it at
fold — the brief scopes this slice to the six packages above), `cargo build --release`,
`cargo run -p a2a-bridge -- validate --repo-hygiene`, `cargo deny`. **Platform exclusions carried
forward:** non-unix `dev`/`ino` (risk R-10 — `validate_object_identity`'s completeness rule and
`observed_identity` both compile to the degraded shape there, untested); Linux-arm execution of the
custody primitives and real-NFS error-after-effect behaviour (carried from the 2b1 and PARKED-1
ledgers; the retried-RPC shape is modelled through `PublicationRenameFaultV1`).

### Mutation checks performed (all reverted before commit)

| Mutation | Expected red | Observed |
|---|---|---|
| delete both pre-add publications in `materialize_under_custody` | `custody_record_is_parent_synced_before_any_git_worktree_add` | RED ✅ |
| `sidecar_file_matches` → always true | sibling-isolation test only | that test RED, under-root test GREEN ✅ |
| `worktree_under_root` → always true | under-root-isolation test only | that test RED, sibling test GREEN ✅ |
| **R1** drop the admitted contract in the binder (the shipped code) | `an_admitted_contract_survives_the_production_authority_binder` | RED ✅ |
| **R2** remove the sweep's publication-cell acquisition | both sweep-arm race tests | 2 RED ✅ |
| **R3** re-add the durable-arm `remove_file` | `a_durable_publication_never_unlinks_the_staging_pathname` | RED ✅ |
| **R4** remove the capability preflight | `a_provider_without_custody_support_publishes_no_record_at_all` | RED ✅ |
| **R8a** revert to last-writer-wins insert | `a_live_custody_record_holds_its_checkout_even_beside_a_free_legacy_sidecar` | RED ✅ |

The last two are the evidence for the 2a carried item: before this slice, neutering either guard
alone left the existing pair green.

---

## 4. Parked findings and deliberate omissions

### 4.1 RESOLVED (was PARKED) — the `Materializing → UnusedSettled` edge is correctly absent

The dual review ruled the shipped protective retention **correct**, and the edge must NOT be added.
Opus's reading, adopted: §5.7 row 3 ("prepared synced, before `git add`") is a CRASH case
recovering from `ProtectionPrepared` — the state 2a's frozen `ProtectionPrepared → UnusedSettled`
edge already serves — so `UnusedSettled` is a **recovery-side** transition, not an in-line writer
transition. 2a's identity data anticipated this arm exactly: `PreservationUnknown
{MaterializationInFlight}` is the ONLY degraded-legal preservation reason, which is precisely the
shape a writer that has already published `Materializing` can produce.

The test is renamed accordingly (repair R9): `add_failure_before_any_target_settles_unused_marker_only`
→ `add_failure_before_any_target_preserves_unknown_and_touches_nothing`, keeping every "only"
assertion. No owner ruling is outstanding.

### 4.2 ACCEPTED, NAMED — a read-only or unwritable `[worktrees].root` now refuses V2 cleanup

The gate's publication cell is created lazily under `<root>/.custody-locks`
(`liveness::open_persistent_lock_file` does `create_dir_all`). If the root exists but the cell
cannot be created, `CheckoutRemovalWindowV1::enter` refuses — fail-closed, a protective leak that
self-heals on the next boot sweep. The one case that would otherwise wedge ordinary cleanup — the
root itself being gone — is explicitly `Ok(None)` (no cell needed; there is no checkout to protect),
matching 2b1's ruling for `probe_custody_record_presence`'s missing-directory arm. This extends
2b1's already-accepted "every V2 cleanup now probes disk" trade.

### 4.2b OWNER ACCEPTANCE REQUIRED — custody lock-file residue is never reclaimed (opus W-1b)

Every custody cell and every publication cell leaves a `<root>/.custody-locks/<id>.lock` file
behind **permanently, by design**: `PersistentLockGuard::drop` releases with `LOCK_UN` and does
NOT unlink, which is the F-3 consolidation and is what makes a stable path safe to share across
acquirers. Unlinking on release is the bug that consolidation fixed — a contender that opened the
doomed inode before the unlink would flock it while a later acquirer creates a fresh one, giving
two holders of one cell on two inodes.

Consequence: one small lock file per checkout per run accumulates in every `[worktrees].root`,
forever. It is bounded per checkout, inert, invisible to both sweep patterns, and now classified
by the storage report (`the_custody_lock_directory_is_classified_and_never_unclassified`) rather
than surfacing as unclassified noise. It is **not** this slice's to solve, and it should not be
solved casually: safe reclamation of a flock'd path is the same problem F-3 already ruled on.

**Ledger item:** reclamation design for `.custody-locks` (flock unlink safety) — needs its own
decision, not an opportunistic fix.

### 4.3 SIZE — the slice exceeded its declared checkpoint

Measured against `main`, **after the repair round**: **1,953 non-test lines added (1,301 code +
652 comment/doc), 3,057 test lines, 5,010 total** across 21 files. (Before the repair round:
1,600 / 2,159 / 3,759.) The brief's checkpoint was "~1,200 non-test / ~3,000 total, stop and
report before continuing".

The first overrun was crossed while completing the last mandated item (S6's storage report), so
there was no remaining scope to stop before. The repair round then added ~1,250 lines — 72% of it
test surface — because five of the eight defects were blocker-class and each required its own
red-first evidence; a declared repair round is not a place to stop for size. Reported rather than
absorbed. The mandated + repair test surface is now ~61% of the diff, consistent with 2b1.

### 4.4 Deliberate non-goals held

No preservation transitions beyond `Materializing → PreservationUnknown` (the add-failure arm the
brief mandates); no barrier placement, no typed retained/refused disposition, no `Reserving`
ownership-retention change (2c1). No `DeleteAuthorized` / `DeletionCapabilityV1` / `remove_v2`
(2c2). No claim exchange (2d). No `workload_identity()` wiring (slice 4). No V2→V3 terminal-row
cutover (slice 5). No `NodeCleanupV2` tightening. Refusal-as-`Ok` remains 2b1's interim projection.

### 4.5 Carried for the fold record

* 2b1's ledger item "hold the refusing custody lock across probe→removal→settlement" is now SPLIT:
  probe→removal landed here (S7); settlement stays 2c1's.
* `§5.7` rows 5 (claim renamed, parent sync ambiguous → preserved) and 6 (claim synced, lease not
  transferred) are 2c1's and 2d's respectively; rows 1–4 are green here.
* The V2 `add`'s `cleanup_failed_add` is UNCHANGED and still deletes — pinned by
  `the_legacy_add_still_removes_the_same_directory`. That is correct (V2 must not change), and it
  means the prohibition holds only for routes that take `add_under_custody`.

---

## 5. §2c SELF-PASS (NOT INDEPENDENT)

**Claim under test:** *the V3 writer is production-unreachable — no production configuration or
admission path can construct or route a V3 custody contract to the backend.*

**Refutation condition:** any production path that mints a `FrozenWorktreeCustodyPlanV1`-bearing
route into `configure_bound_resolved_with_admission` (or any other backend entry) without the
test-only harness.

**Search scope** (workspace-wide `grep -rn --include='*.rs' crates bin`, six independent chokepoints
— the argument only holds if EVERY one is test-only, so each was enumerated separately rather than
inferred from the others):

1. `FrozenR2f1bContractV1::with_computed_fingerprint` and `FrozenR2f1bContractV1 { ... }` literals —
   11 hits, all in `crates/bridge-workflow/tests/{r2f1b_run_spec_v3,r2f1b_workload_identity,
   r2f1a_admission,r2f1a_bound_executor}.rs`, plus 2 doc-comment mentions. Zero production
   constructors.
2. `R2f1bAdmissionV1 { ... }` — 4 hits, all in `tests/r2f1a_{admission,bound_executor}.rs`.
3. `with_frozen_r2f1b_contract` callers — 3, all in `tests/r2f1a_bound_executor.rs`.
4. `bind_custody_plan` callers — `executor.rs:1429` (guarded by `authority.r2f1b.is_some()`, only
   settable via chokepoint 3), `backend.rs:5379` (inside `#[cfg(test)] mod tests`, which starts at
   `backend.rs:2001`), and 4 in `execution_policy.rs`'s own `#[cfg(test)]` module.
5. `BoundWorktreeCustodyV1 { ... }` — `executor.rs:1430` (same guard), `backend.rs:5358` (cfg(test)),
   `custody_writer.rs:525` (cfg(test)), `execution_policy.rs:3070` (cfg(test)).
6. `WorkflowAdmissionRequestV1.r2f1b` — **THREE** production construction sites
   (`coordinator.rs`, `batch.rs`, `main.rs`), all passing `None`. *(Corrected in the repair round,
   opus/sol: the handoff originally said four by counting `detached.rs:7044/7200`, which are
   themselves inside `#[cfg(test)]`.)* `WorkflowAdmissionV1::restore` hardcodes `None`.
   `AdmittedWorkflowRunV1.r2f1b` is now READ by both production consumers through
   `with_admitted_workflow_run` (repair R1) — before that repair it was write-only, which is a
   different and worse defect than unreachability: an admitted contract was silently dropped.

Also checked: `decode_snapshot_v3` / `WorkflowSnapshotV3` still have zero non-test callers
(unchanged from R-10), so no production resume path can carry a contract in; `spec.custody()` has
exactly one production reader (`backend.rs:1237`), so there is no second backend entry.

**Verdict: SURVIVED — with one correction to the claim's phrasing, left visible.**

The claim as stated is about *production paths in this workspace*, and that is exactly what the
evidence supports. It is **not** a type-level impossibility, and it would be wrong to record it as
one: `WorkflowAdmissionV1::freeze` (via a `pub` request field) and
`WorkflowDiagnosticContext::with_frozen_r2f1b_contract` are both `pub`, so an out-of-tree consumer
linking `bridge-workflow` could mint a `ManualOnlyR2f1a` contract and reach the writer. The two
guards that do bind unconditionally are (a) `AutomaticR2f1b` is refused at both boundaries, so no
caller — in-tree or not — can arm a timed run, and (b) nothing production-side mints a
`ManualOnlyR2f1a` V3 contract, which is the precise statement §5.2's ruling relies on. Both halves
of the brief's own formulation ("V3 cannot reach the backend without the routing this slice adds,
and the admission guard refuses `AutomaticR2f1b` while admission mints no `ManualOnlyR2f1a` V3
contract") are therefore satisfied as written.

Second, weaker qualification, recorded because the reader deserves it: unreachability is enforced
by a *call-site* property (four `r2f1b: None` sites) rather than by construction. The field is
explicit and non-defaulted specifically so a fifth site cannot appear without a reviewer seeing it,
but a future production caller passing `Some(..)` would make the writer reachable with no other
change and no test failure. A follow-up slice that wants a stronger guarantee should add a
compile-time or boot-time assertion; this slice does not, and that is a gap, not an oversight.
