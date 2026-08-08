# R2f1b Slice 2 — "Custody + sweep conversion" — implementation brief (rev 2)

**Base:** `main` @ `cffd8e60`, read in the `fold` worktree
(`/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold`). Tree clean; no builds or tests run.

**Revision:** rev 2 folds the dual design review — Sol plan-review ("fix before building", 25 findings /
15 BLOCKER) and the Opus source-verification pass (REVISE-BRIEF). The requirement map is retained and
corrected; the split is restructured. Every contested claim in either review was re-verified against
source before adoption; §8 records what I adopted, what I refined, and the two places I push back with
file:line.

**Authority:** `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` §7 item 2 (§§2.2, 2.5, 5.1,
5.2, 5.7, 5.8, §6 matrix). Ledger: `2026-08-06-r2f1b-pre-slice-2-custody-plan.md` §9;
`reviews/2026-08-08-a4-dual-review.md`; `reviews/2026-08-08-f-slices-review.md`.

**Slice 2 must NOT:** build flight runners (slice 3), timers/deadlines/scheduler (slice 4), construct
`AutomaticR2f1b` (slice 4 — slice 2 adds only the refusal), or do serving-parity work (slice 5).

---

## 0. Verification method and drift

Every citation re-read at `cffd8e60`. Design-doc drift:

| Design doc | Status |
|---|---|
| `backend.rs:865-886` both strengths delete | **Exact.** `run_cleanup_flight` `:817-924`; strength dispatch `:865-874`; removal block `:876-884` |
| `host_git.rs:42` / `:137` / `:147` / `:153-161` | **Exact, all four** |
| `sweep.rs:87` / `:109` | **Exact** |
| `provider_path.rs:67` / `:129` | **Exact** |
| `execution_policy.rs:188` sole activation variant | Drifted: `DeadlineActivationV1` `:209`, `DeadlineActivationV2` `:215` |
| `executor.rs:4619` bare await | Drifted to `:4643`; still unguarded. Irrelevant to slice 2 |
| §2.2 "eight-state" machine | **Ten states** — see R-1 |

**Corrections to rev 1 of this brief (my errors, found by review and confirmed in source):**

1. **The warm-path deletion chain I asserted does not execute in production.** `cleanup_warm_turn`
   (`executor.rs:709-720`) dispatches at `:717` through `Box<dyn NodeTurnCleanup>` (trait
   `executor.rs:386`). That trait has **zero production implementations** — all seven impls are
   `#[cfg(test)]` (`executor.rs:5469`, `:9066`, `:9251`, `:9436`, `:12061`, `:12222`;
   `bridge-a2a-inbound/src/server.rs:874`, gated at `:872`). `release_session_observed(` has **zero
   call sites workspace-wide**. My chain `executor.rs:2677 → cleanup_warm_turn →
   release_session_observed → provider.remove` was **inference, not source.** Corrected in R-9.
2. **My `reap_idle` chain was wrong; the conclusion survives.** `reap_idle`
   (`session_manager.rs:2239-2313`) never calls `release` — the comment at `:2257-2262` says the
   reaper "must SKIP it, never route through `release`". The real chain is `:2311` `claim.cleanup()`
   → `ExpiryClaim::cleanup` `:409` → `start_flight` `:328` → `:355-358`
   `retry.backend.release_session_checked(...)` → `backend.rs:1467` → `:878`. So idle reaping **does**
   reach provider removal, by a different route. The finding stands; the citation is fixed.
3. **`decode_canonical` counts:** production is **13**, not my 9 and not the review's 14. Two of my
   nine were tests (`coordinator.rs:5632` and `workflow_history.rs:3153` are both inside `#[cfg(test)]
   mod tests`); I missed five (`workflow_history.rs:1394`, `:1540`, `:1973`; `sqlite.rs:11088`,
   `:11149`). 24 sites total, 13 production / 11 test.

---

## 1. Requirement map

### R-1 — V3 custody record (§2.2) — **ten states**

`ProtectionPrepared`, `UnusedSettled`, `Materializing`, `LiveProtected`, `PreservationPrepared`,
`Preserved`, `DeleteAuthorized`, `Removed`, `RecoveredLive`, `PreservationUnknown{reason}`
(focused-boundary `:116-123`). Rev 1 said "eight" — corrected, and the complete variant set is pinned
in a wire golden in 2a.

**Today:** `crates/bridge-worktree/src/custody.rs` does not exist.
`PreservedWorktreeClaimV1`, `RecoveryLocatorV1`, `PreservationReasonV1`, `WorktreeCustodyStateV1`,
`DeletionCapabilityV1` — **zero references workspace-wide.** `WorktreeObjectIdentityV1` exists
(`execution_policy.rs:279-283`) with **exactly one reference, its own definition** — never
constructed, never tested. `FrozenWorktreeCustodyPlanV1` (`:286-291`) carries only
`{custody_id, checkout_fingerprint, target_cwd}` — **no source/root/common-dir**, so three of the
claim's four object identities must be captured at materialization by descriptor.

Legacy record: `WorktreeSidecar` (`provider_path.rs:20-29`), 7 flat `String` fields, no identity, no
state, no schema version.

### R-2 — Durable publication (§2.3) — A4 covers reads, **not** replacement

`fs_custody.rs` provides pinning, no-follow open, sync/parent-sync, identity verification, guarded
removal, and crash injection (`PinnedDirectoryV1::open` `:142`, `identity` `:162`, `sync` `:171`,
`sync_journal_recovery_barrier` `:180`, `fail_sync_on_nth_call_for_test` `:184`,
`verify_payload_directory_identity` `:680`, `verify_then_remove` `:765`, `FailureCountdownV1` `:72`).

**Gap confirmed (Sol 5/8):** there is **no descriptor-relative atomic REPLACE primitive.** Every
publication path is no-replace —

```
fs_custody.rs:572   Ok(Some(_)) => return Err(FsCustodyError::TargetExists(...))
fs_custody.rs:466   libc::renameatx_np(..., libc::RENAME_EXCL)        // macos
fs_custody.rs:481   libc::renameat2(..., libc::RENAME_NOREPLACE)      // linux
```

No `std::fs::rename` and no unflagged `renameat*` exists outside the test module. Since **every**
custody transition after `ProtectionPrepared` overwrites an existing record, slice 2 must add a
replace-and-parent-sync primitive. Rev 1's claim that `fs_custody` is complete and untouched was
wrong. Owner: **2b1**.

**Carried (A4):** descriptor-relative recursive deletion stays PARKED (`ReapEnv::remove_tree` swap
point). Off slice 2's path — deletion goes through `git worktree remove`, not `remove_tree`.

### R-3 — Creation ordering (§2.5) — and the missing V3 discriminator

Order is inverted today. Bound path `configure_bound_resolved_with_admission` (`backend.rs:942`):
`provider.add` at `:1023-1026`, `write_sidecar` at `:1048`. Legacy `configure_session` (`:1237`):
`.add` `:1343-1346`, `write_sidecar` `:1371`.

**Gap confirmed (Sol 2):** the bound path **cannot distinguish V2 from V3.**

```
execution_policy.rs:1849  pub struct BoundSessionSpecV1 {
                              pub session: crate::domain::SessionSpec,
                              pub provider_effect: Arc<BoundProviderEffectV1>, }
```

`BoundProviderEffectV1` (`:1821`) → `FrozenProviderAttemptIdentityV1` (`:1722`) →
`FrozenCheckoutEffectV1` (`:1480`). None carries a custody plan or a schema tag.
`FrozenWorktreeCustodyPlanV1` / `FrozenR2f1bContractV1` are reachable only from
`run_spec.rs` `WorkflowSnapshotV3`. So the writer needs **admission → executor → `BoundSessionSpecV1`
propagation** before it can exist. Owner: **2b2**.

**The matching key exists:** `FrozenCheckoutEffectV1::Worktree` carries `checkout_digest`, and
`FrozenWorktreeCustodyPlanV1` carries `checkout_fingerprint` — exact-match plan selection.

### R-4 — Recovery-only sweeps (§5.2)

`sweep_orphans` `:87-101` (`Verdict::Dead` → remove); `WorktreeRunEndGuard::drop` `:109-124`
(`run_id` match → remove, no liveness/outcome check); `remove_worktree` `:18-26`; scanner
`sidecars()` `:70-84` matches **only** `.meta.json` (`:76`).

Guards to preserve verbatim: `sidecar_file_matches` `:28-36` (used `:49`), `worktree_under_root`
`:38-42` (used `:57`).

Install sites, all `bin/a2a-bridge/src/main.rs`: `sweep_orphans` `:3368`, `:3723`, `:4344`, `:7899`,
`:9584`; `WorktreeRunEndGuard` `:3389`, `:3739`, `:4352`. All gated on `wc.enabled`.

**Legacy-`Drop` ruling — rev 1's adjudication is WITHDRAWN, the review's adopted (R9).** There is no
§2.2/§5.2 conflict: §2.2's "existing bounded policy" attaches to the **boot sweep** (confirmed by
§5.2 bullet 7's `legacy-deleted` report category), while §5.2 bullet 2's non-destructive `Drop`
attaches to the **run-end guard**, unqualified. Ruling: **run-end `Drop` becomes non-destructive for
all record kinds** (V2 reclaim defers to the next boot sweep at the five entry points); the boot
sweep's legacy arm stays destructive. Rev 1's argument (c) — "it keeps four tests green" — is struck:
tests staying green is not evidence of design intent. `end_guard_removes_only_this_run`
(`sweep.rs:203`) is **revised** to assert deferred-to-boot reclaim.

### R-5 — Dual-pattern scan (§2.2) and the leak window

`sweep.rs:76` matches one pattern. §2.2: without a dual-pattern scan "V3 checkouts would leak
unreclaimed forever." This is the hard ordering constraint that puts sweep recognition before the
record switch.

### R-6 — Deletion capability (§5.1)

`DeletionCapabilityV1` — zero references. `WorktreeProvider` (`provider.rs:5-16`) = `add` `:8`,
`remove` `:12`, `is_git_repo` `:15`.

**Confirmed (Sol 9/12): exactly NINE `WorktreeProvider` impls** — 1 production
(`HostGitWorktree` `host_git.rs:126`) + 8 test (`workflow_planner.rs:141`; `backend.rs:1836`,
`:1865`, `:1882`, `:1906`, `:3525`, `:3553`, `:3573`). And the provider is type-erased:

```
backend.rs:221   pub struct WorktreeBackend {
backend.rs:222       inner: Arc<dyn AgentBackend>,
backend.rs:223       provider: Arc<dyn WorktreeProvider>,
```

So an inherent `HostGitWorktree` method is **unreachable** from the backend. Both `remove_v2` and the
custody-aware add must be **trait operations with refusing defaults**, enumerated across all nine.

Reusable, as §5.1 claims: `HostGitWorktree::remove` (`host_git.rs:153-171`) already implements the
post-conditions (`:156-159`), with `removal_is_complete` `:95-101`, `registration_absent_from_porcelain`
`:103-110`, `registration_absent` `:112-123`.

### R-7 — `cleanup_failed_add` prohibition (§2.5)

`host_git.rs:42-47` (`remove_dir_all` at `:45`), two call sites `:137`, `:147`, both inside
`HostGitWorktree::add` (`:127-151`). Because `add` is a trait method serving V2 and V3 alike,
prohibition requires an **explicit custody-aware trait operation**, not a change to `add`. Owner:
**2b2**.

### R-8 — `preserve_after_cancel` (§5.1)

Both strengths delete (`backend.rs:865-874` dispatch, `:876-884` removal). `CleanupStrength` is
`{Forget, Release}` (`:50-54`, `Ord`). `run_cleanup_flight` **has no notion of why cleanup was
requested.** Every failure/cancel path hardcodes `Release`.

**Cleanup single-flight keying (Sol 11) — refined.** The join key is `(session cell, strength
ordering)` and nothing else: `claim_cleanup_cell` selects by `session.as_str()`, and
`start_or_join_cleanup` (`:555-571`) reuses on `existing.strength >= requested`.
`CleanupFlightSlot` (`:87-93`) = `{id, strength, report, [test] joined_waiters}`;
`CleanupCellState` (`:72-78`) = `{inner_strength, provider_removed, sidecar_removed, entry}`.
**But there is no preservation request today**, and both strengths reach the same unconditional
removal — so the wrong-join defect is **prospective, not current.** It becomes real the moment a
preserve disposition is added. It is therefore a **design requirement on 2c1**, not a present bug.

**Cancellation ordering (Sol 16) — CONFIRMED and it moves work out of `run_cleanup_flight`.**
Cold paths call `cancel_observed` **before** cleanup:

```
executor.rs:3340-3345   .cancel_observed(&session, diagnostic.clone()).await.err();
executor.rs:3347-3353   cleanup_cold_session(..., ColdCleanupAction::Forget, ...)
executor.rs:3491-3497   cancel_observed  (drain-cancel path)
executor.rs:3542-3551   cleanup_cold_session
```

Per §5.1 step 6 preservation must precede the session/resource signal, so the preservation barrier
goes **before `cancel_observed` at `:3340` and `:3491`** — inserting it only inside
`run_cleanup_flight` runs too late. A third site, `executor.rs:3513-3521`, destroys the session with
no preceding `cancel_observed` (rich-flush-failure) and must also be enumerated.

### R-9 — Workflow-level disposition (§5.1) — **corrected**

Rev 1 claimed the warm chain reaches `provider.remove` in production. It does not (§0 correction 1).
The accurate statement:

- **Warm node-end cleanup has no production implementation at all** — `NodeTurnCleanup`
  (`executor.rs:386`) has zero production impls; `release_session_observed` has zero call sites.
- **Cold node-end cleanup is real**: `cleanup_cold_session` (`executor.rs:966-987`) calls
  `forget_session_observed` `:978` / `release_session_observed` `:983`… which, per the above, have no
  production callers either. The live production deletion pressure on worktrees comes from
  **`SessionManager`** (R-11), not from the executor's node loop.
- **No global-outcome gate exists.** `WorkflowCleanupTracker` (`executor.rs:599-618`) is
  observability-only, read post-loop at `:5008`.

**Extend the tracker, do not build a parallel registry (adopted).** It is already
`Arc<WorkflowCleanupTracker>` (`executor.rs:4270`), already `Mutex`-interior (`:600`), already keyed
`BTreeMap<NodeId, NodeCleanupState>` (`:611`), already written from inside node futures
(`record` `:621-641`, cloned in at `:4606`, passed at `:4623`), already read per-node at `:4736` and
post-loop at `:5008`. Adding `checkout: Option<CheckoutDisposition>` to `NodeCleanupState` plus
`record_checkout()` needs no new plumbing across its ~45 call sites.

**The real deferral objections (adopted, replacing the struck `:2957` argument):**

```
executor.rs:2677-2694   cleanup failure is folded into the node's OWN terminal outcome
                        (ok / text / node_outcome / primary_error)
executor.rs:3097-3116   let cleanup_allows_retry = cleanup_cold_session(...).await.is_ok();
                        break 'attempt Attempt::Transient { ..., cleanup_allows_retry, ... }
```

Both sites need a **completed** disposition at node-end — one to classify the node terminal, one to
gate the next attempt. This argues for deferring **only terminal checkout disposition**, keeping
session teardown node-local. That is the surgical cut, and it is what 2c2 implements.

### R-10 — Claim exchange on resume (§5.8) — no production owner exists

`WorkflowSnapshotV3` is entirely production-unwired; `decode_snapshot_v3` (`run_spec.rs:500`) has
**zero non-test callers** (only `tests/r2f1b_run_spec_v3.rs:149,386,406` and
`tests/r2f1b_workload_identity.rs:194`).

**Confirmed (Sol 13/15):** `run_workflow_cmd` (`main.rs:4103`) has **no resume path** — it always
mints a fresh identity (`main.rs:4120` `AttemptIdentity::initial()?`). `implement_resume_cmd`
(`:3608`) resumes an **implement checkpoint**, reconstructing from config + checkpoint
(`:3636-3639`, `:3651`), never from a serialized snapshot. Rev 1's 2d wiring targets do not exist.
Re-scoped in §3.

Reusable and contract-tested: `WorkflowSnapshotV3::validate` `run_spec.rs:98-122`,
`validate_successor` `:159-183`, `decode` `:140` (canonical re-encode check `:153`).

### R-11 — Deletion families reaching `backend.rs:876-884`

The removal block lives only in `run_cleanup_flight`, spawned from exactly two sites
(`backend.rs:617` initial, `:747` failed-configure re-spawn), reached from
`start_or_join_cleanup`'s exactly **three** callers (`:177`, `:524`, `:782`). **This convergence is
what makes a single fail-closed gate sufficient** — the whole fan-in funnels through one block.

In-crate families (7): configure-admission rollback (`ConfigureAdmission::Drop` `:177`);
bound-configure rollback (`:991`, `:1033`, `:1052`, `:1064`, `:1089`, `:1100`); legacy-configure
rollback (`:1311`, `:1356`, `:1375`, `:1386`, `:1399`, `:1427`, `:1438`); retirement (`:1571`);
forget (`:1444`/`:1448`); release (`:1461`/`:1467`); observed cleanup (`:1452`/`:1472`).

External subsystems (5): **`BindingGuard::Drop`** (`bridge-coordinator/src/dispatch.rs:48-64`,
`forget_session` at `:63`); **`ExpiryClaim`** — 12 call sites across three entry APIs plus `Drop`
(`into_flight` `session_manager.rs:851`, `:918`, `:963`, `:1028`, `dispatch.rs:277`, `:349`;
`cleanup()` `session_manager.rs:1406`, `:1578`, `:1603`, `:1775`, `:1933`, `:2312`; `Drop`
`:420-423`); **direct `release_session`** — exactly **11** non-test sites (`session_manager.rs:1101`,
`:1206`, `:1224`, `:1352`, `:2015`, `:2025`, `:2036`, `:2162`, `:2170`, `:2181`, `:2228`);
**workflow cold cleanup** (`executor.rs:966-987`); **controller retire**
(`bridge-controller/src/resilient.rs:71`, `:178`, `:192`).

"Eight families" is a defensible taxonomy; the layered structure above is the accurate one and is the
test surface for 2b1's gate.

### R-12 — Ledger items confirmed

- **`workload_identity()` wiring is slice 4's.** Custody plan §9 (A5) and focused boundary §7 item 4
  agree. Slice 2 must not touch `batch.rs:1090` / `coordinator.rs:1646` / `main.rs:4285`.
- **Six-enum rule does not bind slice 2.** `NodeCauseV1` is a struct, not one of the six. Do not
  opportunistically tighten `NodeCleanupV2`.
- **SIGCHLD residual** = a test-flake item (`reviews/2026-08-08-flake-fix-review.md:37`), not a
  custody obligation. Recorded, not scheduled.

---

## 2. Split

Six sub-slices. The coordinator's five-way structure is preserved in substance; **2b is cut in two on
sizing evidence** — see the pushback in §8.3.

```
2a    read-only classification + refusals      (decoder, wire contract, sweep recognition, NodeCauseV1)
2b1   protection primitives + fail-closed gate (replace primitive, custody lock, deletion refusal)
2b2   V3 routing + writer + creation ordering  (propagation, publication, add-prohibition, refusal)
2c1   fail-closed preservation                 (leaks-but-preserves; mints no deletion authority)
2c2   deletion capability                      (DeleteAuthorized, remove_v2, post-loop mint)
2d    claim-exchange mechanism                 (production-inactive)
```

**Coupling that drives the order:**

1. **Leak window (R-5).** Switching the record name without dual-pattern recognition leaks forever ⇒
   recognition (2a) precedes the writer (2b2).
2. **Protection precedes the writer (R1 ruling, strengthened).** The fail-closed deletion gate lands
   in 2b1, *before* any V3 record can exist — strictly safer than landing it alongside the writer,
   and it satisfies the ruling's no-window requirement a fortiori.
3. **Replace primitive and lock precede every transition (R-2, Sol 14).** No custody state can be
   written twice without them ⇒ 2b1.
4. **Routing precedes the writer (R-3).** `BoundSessionSpecV1` cannot distinguish V2 from V3, so
   propagation is a precondition of a V3-only writer ⇒ 2b2.
5. **Preservation and deletion are separable (R2 ruling, adopted).** Rev 1's atomicity argument rested
   on `backend.rs:2957`, which measures only the 64 configure permits — struck. A deletion-refusing,
   temporarily-leaking intermediate is viable and safer ⇒ 2c1 / 2c2.
6. **`cleanup_failed_add` sits in the add path (R-7)** ⇒ 2b2, with creation ordering.

---

## 3. Per sub-slice

Each sub-slice is one branch and one PR with a **declared one-round review cap**; closed-enumerable
findings get one targeted repair, open-class findings park and escalate. Each is written as ordered
TDD steps: **red → implement → ripple → gates → commit.**

### 2a — Read-only classification and refusals

**Scope.** Decoder and wire contract only. **No write primitive, therefore no settlement** — Sol 1's
contradiction (2a was assigned run-end settlement while forbidden publication) is resolved by moving
durable settlement to 2b2, where the replace primitive exists.

**Wire contract to specify at compile level in the PR (Sol 19).** `custody.rs` must define, before any
consumer exists: the ten-variant `WorktreeCustodyStateV1` with concrete serde tags; the schema
constant; `PreservedWorktreeClaimV1` field-for-field with bounds on every string; the
`PreservationReasonV1` and `RecoveryLocatorV1` variant sets; the decoder error type; and the legal
transition table as data. Golden wire bytes pin all ten variants so later exhaustive matches cannot
silently diverge between 2a's reader, 2b2's writer, and 2d's exchange.

**Steps**
1. **Red:** golden-bytes tests for all ten states + claim round-trip; decoder-rejection tests (unknown
   variant, out-of-bounds string, missing schema, wrong schema version).
2. **Red:** sweep tests — V3 record recognized and excluded; corrupt/missing/mismatched/symlinked/
   multi-link → unknown-never-deleted; legacy boot arm byte-for-byte unchanged; run-end `Drop`
   non-destructive for **all** record kinds (R9) with deferred-to-boot reclaim asserted.
3. **Red:** `node_cause_rejects_unknown_nested_field`; flip the two A2 FINDING tests
   (`resource_flight.rs:588-597`, `preparation_flight.rs:465-466`) from `is_ok()` to `is_err()`.
4. **Implement:** `custody.rs` types + decoder; `sweep.rs` dual-pattern recognition with the V3 arm
   recovery-only; `NodeCauseV1` `deny_unknown_fields`.
5. **Ripple:** revise `end_guard_removes_only_this_run` (`sweep.rs:203`) per R9. Preserve
   `sweep.rs:177`, `:226`, `:258` unchanged.
6. **Gates → commit.**

**Files:** `bridge-worktree/src/{custody.rs (new), sweep.rs, lib.rs}`;
`bridge-core/src/{execution_policy.rs, resource_flight.rs, preparation_flight.rs}`.

**Size:** ~900–1,100 LOC, of which ~450 is declarative type/golden content. (Rev 1's 550–750 was
below its own component sum — Sol 21 correct.)

**Lens:** **dual** — modifies code with deletion authority even though the intent is to narrow it.

**Non-goals:** no publication, no settlement, no lock, no writer, no preservation, no flight runners,
no timers, no `workload_identity()` wiring.

---

### 2b1 — Protection primitives and the fail-closed deletion gate

**Scope.** Everything that must exist *before* a V3 record can be written: atomic replacement, the
custody lock, and one strength-independent deletion refusal.

**The gate (R1, adopted).** One fail-closed check at the removal block `backend.rs:876-884`,
consulting custody state reachable from `WtEntry`. This works because `WtEntry` is exactly the value
that flows there:

```
backend.rs:43-47   struct WtEntry { canonical_source: String, worktree_path: String }
```

Four construction sites — `:950` and `:1078` (bound, inside `configure_bound_resolved_with_admission`
fn `:942`) and `:1269`, `:1413` (legacy, inside `configure_session` fn `:1237`).
`entry_for_cleanup` (`:488-503`) clones it into `state.entry` (`:861-863`), and `:876-884`
dereferences it. A discriminator set at `:950`/`:1078` is visible verbatim at the gate.
`WtEntry` derives only `Clone`; the field-by-field comparison at `:903-908` needs no change.

Placing the gate here is what makes it strength-independent and context-free — `ExpiryClaim::Drop`,
`BindingGuard::Drop`, and the reaper have **no workflow context** to consult, but they all funnel
through this one block (R-11).

**The custody lock (Sol 14).** Reuse the existing primitive rather than inventing one:

```
liveness.rs:160  pub struct PersistentLockGuard { path: PathBuf, _file: std::fs::File }
liveness.rs:201  acquire_persistent_lock_in(dir, lock_id) -> io::Result<PersistentLockGuard>
liveness.rs:227  acquire_persistent_lock_blocking_in(dir, lock_id, on_contended)
```

Keyed `(dir, lock_id)` → `<dir>/<lock_id>.lock`, arbitrary string, `Drop` releases without unlinking
(the F-3 consolidation). Specify: `custody_id`-derived `lock_id`, the directory, acquisition order
relative to the existing global lock ordering obligation documented on
`acquire_persistent_lock_blocking_in`, contention result, and retained-guard owner.

**Steps**
1. **Red:** `fs_custody` replace-and-parent-sync tests including the **ambiguous post-rename-sync**
   fault (record replaced, parent sync fails → typed ambiguous outcome, protective).
2. **Red:** custody-lock tests — writer-vs-sweep and preserve-vs-delete race, both orders.
3. **Red:** the gate's test surface — **every** family in R-11 must be proved unable to delete a
   protected entry: `ConfigureAdmission::Drop`; bound and legacy configure rollback; `retire()`;
   forget/release/observed; `BindingGuard::Drop` (`dispatch.rs:63`); `ExpiryClaim` via
   `cleanup()`/`into_flight()`/`Drop`; the 11 direct `release_session` sites; workflow cold cleanup;
   controller retire. Plus an end-to-end `SessionManager` test through the real `reap_idle` →
   `claim.cleanup()` → `start_flight` chain.
4. **Implement:** replace primitive in `fs_custody.rs`; custody lock module; `WtEntry` discriminator
   + gate at `:876-884`.
5. **Ripple:** the 13 legacy tests stay green untouched (R3 — see §8.1).
6. **Gates → commit.**

**Files:** `bridge-core/src/fs_custody.rs`; `bridge-worktree/src/{custody.rs, backend.rs}`;
tests in `bridge-coordinator`.

**Size:** ~700–850 LOC.

**Lens:** **dual** — this is the deletion-authority gate.

**Non-goals:** no record writer, no routing, no preservation transitions, no deletion capability.

---

### 2b2 — V3 routing, the writer, and creation ordering

**Scope.** The first V3 writer, and everything it needs to be reachable *by tests* while remaining
**production-unreachable** (see the R6 adjudication, §5.2).

**Routing (Sol 2).** Specify admission → executor → `BoundSessionSpecV1` propagation carrying the
selected `FrozenWorktreeCustodyPlanV1`, matched **exactly** on
`FrozenCheckoutEffectV1::Worktree.checkout_digest` == `FrozenWorktreeCustodyPlanV1.checkout_fingerprint`.
V2 routes must be byte-identical; V2/V3 routing tests both directions.

**`AutomaticR2f1b` refusal placement (Sol 13).** **Not** in `FrozenR2f1bContractV1::validate` — that
would make `with_computed_fingerprint` fail and break the existing A3/A5 offline construction and
workload-identity tests. Place it at the **production admission boundary of the first V3 consumer**,
which this sub-slice creates. Offline construction, encoding, and decoding stay legal.

**Custody-aware add (Sol 9).** An explicit trait operation on `WorktreeProvider` with a **refusing
default**, enumerated across all nine impls (R-6). Do not change `add` — it serves V2.

**Steps**
1. **Red:** routing tests (V3 selects the matching plan; digest mismatch refuses; V2 unchanged).
2. **Red:** ordering — `custody_record_is_parent_synced_before_any_git_worktree_add`; crash matrix
   §5.7 rows 1–4 using `fail_sync_on_nth_call_for_test` / `FailureCountdownV1`.
3. **Red:** `add_failure_after_target_creation_never_removes_target`;
   `add_failure_before_any_target_settles_unused_marker_only`;
   `partial_add_publishes_preservation_unknown_materialization_inflight`.
4. **Red:** `v3_path_writes_no_legacy_meta_json`; `old_binary_sweep_cannot_select_a_v3_checkout`.
5. **Red (Sol 23):** V3 storage-report test — Evidence classification, sibling association, holder
   state per custody state, and unchanged V2 output.
6. **Red:** `automatic_r2f1b_refused_at_production_admission` + `manual_only_r2f1a_admitted` +
   `offline_automatic_contract_construction_still_legal` (guards the A3/A5 regression).
7. **Red:** durable run-end settlement (moved here from 2a) — explicit idempotent `settle()` on
   normal/handled terminal paths at the five entry points, with abrupt `Drop` proved protective and
   **not** pretending settlement occurred (Sol 17).
8. **Implement:** routing; publication + ordering inversion; custody-aware add; refusal; settlement;
   `storage_report.rs` second suffix (`:1546`, `:1582`).
9. **Ripple:** V3 twins for `backend.rs:2060`, `:2135`; all V2 originals unchanged.
10. **Gates → commit.**

**Files:** `bridge-workflow/src/{admission.rs, executor.rs}`; `bridge-core/src/execution_policy.rs`;
`bridge-worktree/src/{custody.rs, backend.rs, provider.rs, host_git.rs, sweep.rs}`;
`bin/a2a-bridge/src/{main.rs, storage_report.rs}`.

**Size:** ~900–1,100 LOC.

**Lens:** **dual** — removes a deletion call and establishes the ordering every crash property rests on.

**Non-goals:** no preservation policy, no deletion capability, no resume, no flight runner, no timers.

---

### 2c1 — Fail-closed preservation

**Scope.** V3 failure/cancel produces durable `PreservationPrepared` → `Preserved` /
`PreservationUnknown`. **Nothing mints deletion authority.** V3 checkouts leak for the workflow's
lifetime — deliberately, and safer than irreversible deletion (adopted from the review's adjudication).

**Preservation barrier placement (R-8).** Before `cancel_observed`, not inside `run_cleanup_flight`:
`executor.rs:3340`, `:3491`, plus the no-cancel destroy at `:3513-3521`, plus the warm, preflight, and
`SessionManager` entry points enumerated in R-11.

**Single-flight keying (R-8).** Extend `CleanupFlightSlot` / `CleanupCellState` / the retry state with
a monotonic custody disposition identity so a preservation request and a later deletion request of
equal strength cannot join the wrong flight. Test both race orders. (Prospective defect — it becomes
reachable in this sub-slice.)

**Steps**
1. **Red:** `failure_cancel_and_ambiguity_never_call_provider_remove_reset_clean_prune`;
   `preservation_precedes_cancel_observed_at_every_enumerated_entry` (ordering witness per site).
2. **Red:** `claim_renamed_with_ambiguous_parent_sync_stays_protective` (§5.7 row 5);
   `preserved_claim_awaits_r2f2_with_no_provider_replay` (row 12).
3. **Red:** single-flight race tests, both orders.
4. **Red:** V2 behaviour byte-identical (positive control).
5. **Implement:** preservation transitions; barrier insertion; flight-key extension.
6. **Ripple:** V3 twins only; the 13 legacy tests untouched (§8.1).
7. **Gates → commit.**

**Size:** ~800–950 LOC. **Lens: dual** (preservation authority).

**Non-goals:** no `DeleteAuthorized`, no `DeletionCapabilityV1`, no `remove_v2`, no post-loop mint.

---

### 2c2 — Deletion capability

**Scope.** `DeleteAuthorized` CAS, `DeletionCapabilityV1`, `remove_v2`, and the post-loop mint on an
all-healthy outcome. This closes 2c1's deliberate leak.

**Drain path (Sol 6).** The executor holds `Arc<dyn AgentBackend>` and `Box<dyn NodeTurnCleanup>`; it
cannot reach `WorktreeProvider`. Add an explicit **refusing-default disposition API** and enumerate
every forwarding wrapper and double, or carry a capability-bound disposition handle through
`NodeTurnCleanup` and all seven of its (test-only) impls. Rev 1's "no `AgentBackend` trait changes"
non-goal is **withdrawn for 2c2** — it made the drain impossible.

**Extend `WorkflowCleanupTracker`** (R-9) rather than building a parallel registry; mint at
`executor.rs:5008` on an all-healthy outcome, otherwise run preservation for every materialized
checkout.

**`remove_v2` (Sol 10).** Capability-consuming signature with a refusing default across all nine
`WorktreeProvider` impls; name which fakes override it for the success/identity tests.

**Steps**
1. **Red:** `node_local_success_cannot_remove_its_checkout`;
   `global_healthy_success_with_capability_removes_exactly_once`;
   `completed_sibling_survives_later_workflow_failure` (outcome-driven — see §6 note).
2. **Red (Sol 24) — failure boundaries, each with a defined recovery-owned/preserved/unknown result:**
   crash after `DeleteAuthorized`; git remove failure; prune failure; post-condition disagreement;
   parent-sync failure while recording `Removed`.
3. **Red:** `remove_v2_refuses_when_object_identity_changed_since_authorization`;
   `raw_path_removal_is_unreachable_without_a_capability`.
4. **Implement:** CAS + capability + `remove_v2` + disposition API + tracker extension + post-loop mint.
5. **Ripple:** nine `WorktreeProvider` impls; every `NodeTurnCleanup` impl; forwarding wrappers.
6. **Gates → commit.**

**Size:** ~700–900 LOC. **Lens: dual** (deletion authority).

**Non-goals:** no timers or cutoffs (slice 4), no resource-flight runners (slice 3), no serving parity.

---

### 2d — Claim-exchange mechanism (production-inactive)

**Re-scoped (R7).** No durable V3 resume owner exists (R-10), so 2d delivers the **mechanism plus
tests, production-inactive until slice 5's serving parity**, which §7 item 5 already owns ("detached
sink, terminal CAS, coordinator, A2A, MCP, batch, **resume**"). Rev 1's `main.rs` wiring targets are
struck.

**Defers:** the production half of the §6 "Exact resume exchange" row. The mechanism half — successor
minting, claim validation, `RecoveredLive` publication, sweep exclusion — lands here and is fully
testable; the "precedes resume provider effect" half needs a production resume path and moves to
slice 5.

**Steps**
1. **Red:** `successor_attempt_and_claim_exchange_validates_before_any_provider_call`;
   negatives — reused current attempt, wrong origin/digest/lineage/parent.
2. **Red:** `crash_after_claim_exchange_remains_recovered_live_and_sweep_protected`;
   `claim_synced_but_lease_untransferred_keeps_both_protections` (§5.7 row 6);
   `terminal_preserved_claim_is_not_exchanged`.
3. **Implement:** `RecoveredLive` transition; recovery-lease acquisition; first production-shaped
   consumer of `validate_successor` (`run_spec.rs:159`), exercised by tests.
4. **Gates → commit.**

**Size:** ~600–750 LOC. **Lens: dual recommended** — `RecoveredLive` must inherit `LiveProtected`'s
sweep exclusion, and a miss there is a silent deletion path spanning 2a and 2d.

**Gate for slice 3:** §7 item 2 requires "all worktree crash tests green before continuing" — §5.7
rows 1–6 and 12, green at the end of 2d. Rows 7–11 are slice 3's.

**Total slice 2: ~4,600–5,650 LOC across six PRs.**

---

## 4. Risks and disposition

| # | Risk | Trigger / likelihood | Impact | Disposition |
|---|---|---|---|---|
| R-1 | Leak window — record name switched before recognition | Certain if ordered wrong | Unreclaimable V3 checkouts | **RESOLVED BY SPLIT** — 2a precedes 2b2 |
| R-2 | Deletion reaches a protected V3 entry from a context-free path | `ExpiryClaim::Drop`, `BindingGuard::Drop`, reaper; routine | Irreversible loss of preserved work | **RESOLVED BY 2b1** — one gate at `backend.rs:876-884`, all R-11 families as test surface |
| R-3 | 2c1's deliberate leak persists if 2c2 slips | Certain between the two PRs | Disk growth; no wrong output | **ACCEPT** — safer than deletion (adjudicated). 2c2 must not be deferred past slice 3 |
| R-4 | Storage report blind to V3 | Certain once 2b2 switches the name | Report quality only | **CLOSE IN 2b2.** *Verified not a deletion hazard*: `ItemSource::WorktreePath` is assigned by root membership (`storage_report.rs:1593`, `:1645`), not sidecar presence, and `storage_reap.rs:1112-1116` refuses that source outright |
| R-5 | AgentBackend double churn | Slice 3 | Large mechanical diff | **DOES NOT HIT 2a–2c1.** 119 impls / **114 doubles** / 5 production; trait `ports.rs:153-285` has only 2 non-defaulted methods, so a defaulted addition costs 0. **2c2 does touch `NodeTurnCleanup`** (7 impls, all test-only) — small and enumerated |
| R-6 | Peak checkout count rises — checkouts held to workflow end | Every multi-node workflow | Disk pressure | **ACCEPT; not a gate.** Rev 1 cited `backend.rs:2957` — struck. The real bound is `MAX_WORKTREE_CONFIGURES_IN_FLIGHT = 64` (`backend.rs:25`, enforced `:313`), which bounds **concurrent configure calls, not live checkouts**: the permit is released in `ConfigureAdmission::Drop` (`:508`) even on success, so materialized worktrees are unbounded by it. Cite that as the real deferral cost. Per Sol 25, either a deterministic provider-free fixture with a stated expected peak, or dropped as a gate — **dropped**, recorded as a slice-4 capacity question |
| R-7 | Wrong-flight join between preserve and delete | 2c1 onward | Wrong cleanup result shared | **DESIGN REQUIREMENT ON 2c1** — prospective, not a current defect (R-8) |
| R-8 | Three of four claim identities absent from the frozen plan | 2b2; certain | Claim not constructible from contract alone | **ACCEPT** — capture at materialization by descriptor |
| R-9 | Descriptor-relative `remove_tree` parked (A4) | Hostile concurrent host actor; rare | TOCTOU | **DEFER — off path.** Deletion goes through `git worktree remove` |
| R-10 | `dev/ino` on non-unix | Windows; unsupported | N/A | **DEFER** — name the exclusion in each gate report |

### Rollback — the specific question

**Already guaranteed by construction, with one condition.** The legacy scanner is `sweep.rs:76`
(`ends_with(".meta.json")`); `remove_worktree` (`:18-26`) is reachable only through `sidecars()`. An
old binary never enumerates a `.custody.v1.json`, so it cannot select the worktree.

**The condition:** the V3 path must not *also* write `.meta.json`. §2.2 forbids it. 2b2 adds **no
rollback code** — it adds `old_binary_sweep_cannot_select_a_v3_checkout` (pins the guarantee) and
`v3_path_writes_no_legacy_meta_json` (pins the condition).

---

## 5. Owner decisions and adjudications

### 5.1 — `NodeCauseV1` `deny_unknown_fields`: **RECOMMEND TIGHTEN, in 2a** (unchanged; citations corrected)

`BoundedCauseV1 = NodeCauseV1` (`execution_policy.rs:714`), nested at `NodeTerminalV1.cause` `:603`,
`NodePrimaryRecordV3.cause` `:722`, `NodeCleanupV2::Failed.cause` `:855`,
`ResourceActionResultV1.cause` (`resource_flight.rs:167`), `PreparationFlightStateV1` cause
(`preparation_flight.rs:122`). `NodeCauseV1` (`:486-493`) carries no `deny_unknown_fields`.

**On the live persisted path the tightening is behaviourally neutral — a stricter check already runs:**

```
execution_policy.rs:705-709   let value: Self = serde_json::from_slice(bytes)…;
                              if value.encode_canonical()? != bytes {
                                  return Err(ExecutionPolicyError::InvalidStructuredEvidence);
```

An unknown nested field is dropped on deserialize, so the re-encode cannot reproduce the input bytes
and the decode **already fails**, with the identical error. The re-encode makes the persisted form a
**frozen byte contract**, not merely a field-set contract. Same for `NodePrimaryRecordV3`
(`:783-793`) and `NodeCleanupRecordV2` (`:1059-1069`).

**All 13 production decodes go through `decode_canonical`** — `detached.rs:2957`, `:4416`, `:4882`;
`workflow_history.rs:1394`, `:1540`, `:1973`; `task_store.rs:2200`; `sqlite.rs:5520`, `:11088`,
`:11149`, `:11316`, `:13453`; `main.rs:4746`. **Zero plain-deserialize sites.** So there is no live
rollback tolerance to remove.

**The one path where it matters** is the flight types — `ResourceActionResultV1` /
`ResourceFlightStateV1::Settled` / `PreparationFlightStateV1` — plain serde, no re-encode check, with
A2's two `is_ok()` FINDING tests pinning the gap (`resource_flight.rs:588-597`,
`preparation_flight.rs:465-466`). **Zero production writers today**; slice 3 journals them.

**Recommendation: tighten now, in 2a** — free on live paths, closes a recorded gap before the path
gains writers, same window and precedent as F-2's unit-variant tightening. Work: one attribute, two
flipped tests, one positive control. **Scope guard:** `NodeCauseV1` is a struct, so the six-enum rule
does not apply; do **not** tighten `NodeCleanupV2` (already protected by `NodeCleanupRecordV2`'s own
`deny_unknown_fields` `:871` plus `decode_canonical` `:1059-1069`, and tightening it *would* trigger
the rule).

**What would change this:** a production path deserializing `NodeCauseV1` without a canonical
re-encode check, over already-written bytes carrying extra fields. I searched; none exists.

### 5.2 — R6 ADJUDICATION: preparation-flight sequencing — **inactive-writer pattern HOLDS**

§2.5 requires a bounded, independently owned preparation flight before the first custody open, lock,
write, rename, or sync. No slice owns the runner: `preparation_flight.rs` is inert contracts
(`PreparationFlightIdV1` `:15`, `BoundedPreparationTransferReasonV1` `:77`,
`PreparationFlightStateV1` `:115`, `PreparationClockV1` `:127`), and §7 assigns runners to slice 3.

**Ruling — adopt the inactive-foundation pattern, with the runner assigned to slice 3:**

1. 2b2's writer lands **production-unreachable**. This is enforceable, not aspirational: V3 cannot
   reach the backend without the routing 2b2 itself adds, and 2b2's admission guard refuses
   `AutomaticR2f1b` while admission mints no `ManualOnlyR2f1a` V3 contract.
2. The preparation runner and transfer behaviour are **explicitly slice 3's**.
3. The first **production-reachable** V3 write happens in the slice holding both the runner and the
   gate — slice 5 at the earliest, per §7 item 5's ownership of resume and serving surfaces.

**Safety argument:** no timers exist until slice 4, so the unbounded-stall hazard §2.5's flight guards
against cannot manifest through slice 2; and cancellation-abandonment is guarded by unreachability.

**The coordinator asked whether 2c1/2c2 break this by needing reachable writes to test preservation
end-to-end. They do not — source is decisive.** The entire node-end cleanup surface is *already*
test-driven: `NodeTurnCleanup` has zero production impls (`executor.rs:386`), and
`release_session_observed` has zero call sites workspace-wide. Preservation is therefore tested the
same way the existing ~40 `backend.rs` cleanup tests are — by driving `WorktreeBackend` and
`SessionManager` directly. **No minimal runner is needed in 2b.**

### 5.3 — R8 ADJUDICATION: durable sink — **neither option; §7 item 5 already owns it**

Confirmed: `reserve_node_terminal_rows_v3` (`task_store.rs:671`, impls `:2324`, `sqlite.rs:5691`),
`put_node_primary_sequenced_v3` (`:680`, `:2386`, `:5777`), `settle_node_cleanup_sequenced_v3`
(`:692`, `:2436`, `:5850`) — **all callers are tests, zero production.** Production writes node
terminals via `commit_node_terminal_v2` (`detached.rs:673`; `main.rs:4597`, `:4715`) and
`put_node_checkpoint_sequenced_v2` (`detached.rs:640`, `:3874`).

**Ruling: neither "carry the cutover in 2c1" nor "block on a precursor PR."** Both presuppose slice 2
needs a *production* write path for preservation results. Under 5.2's ruling it does not — V3 stays
production-unreachable through slice 2, so preservation results are exercised through the existing,
already-test-covered v3 store functions. The V2→V3 terminal-row cutover belongs to the slice that
first makes V3 production-reachable, and **§7 item 5 already names it**: "Persistence + serving
parity. Detached sink, **terminal CAS**, coordinator, A2A, MCP, batch, resume."

**Recorded as a named dependency, not a slice-2 blocker.** If the owner wants it earlier it is a
bounded precursor PR (~200–300 LOC: route the three v3 functions into `detached.rs:640/673` and
`main.rs:4597/4715` behind the V3 discriminator). Flagged so it cannot be silently dropped.

### 5.4 — No other owner decisions

`workload_identity()` wiring is slice 4's by agreement of both documents; the `AutomaticR2f1b`
refusal is a mandated obligation with a now-specified placement; the SIGCHLD residual is a test-flake
item.

---

## 6. §6 matrix ownership

Every normative row gets exactly one owner. **Scope note:** the focused boundary is a frozen,
checked-in design document and my task authorizes only this scratchpad brief — so this table is the
authoritative slice-2 assignment, and propagating it into the frozen doc's §6 requires an
owner-authorized amendment. The exact edits are specified below so the apply is mechanical.

| §6 row | Owner | Note |
|---|---|---|
| Protection precedes clocks/effects | **split, declared** | effects half = 2b2; **activation/clock half = slice 4** |
| Preparation is finitely owned | **slice 3** | per 5.2; the orphaned row now has an owner |
| Both sweeps exclude protection | **2a** | |
| Partial add preserved | **2b2** | |
| Candidate settlement | **2d** (whole row) | the 2a unit test is reclassified a **prerequisite**, not a row claim (Sol 8/17) |
| Exact resume exchange | **2d mechanism / slice 5 production** | half deferred, declared |
| Cancel cannot delete | **2c2** | outcome-driven; the **timer-driven cutoff variant is slice 4's** |
| Destructive wrappers join flight | **2b1 gate satisfies most, in 2b1**; **flight-JOIN semantics stay slice 3** | |
| Byte budget / terminal monotonicity | slice 1 (landed) | |
| Rollback | **2a + 2b2** | |

**Amendment text for the frozen doc, if authorized:** in §6, annotate the "Cancel cannot delete" row
with "(timer-driven cutoff variant: slice 4; slice 2 lands the outcome-driven equivalent)"; annotate
"Protection precedes clocks/effects" with "(effects half: slice 2; activation half: slice 4)"; and
annotate "Exact resume exchange" with "(mechanism: slice 2; production resume path: slice 5)".

---

## 7. Gates

Per sub-slice and aggregate:

```bash
git diff --check
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --bin a2a-bridge
cargo run -p a2a-bridge -- validate --repo-hygiene
```

Report exact passed/failed/ignored totals; name every excluded platform (R-10) and every check the
environment could not run. No provider, compatibility, smoke, release, or deployment step is in these
gates. **One-round review cap per sub-slice.**

Standing rule (custody plan §4): **a fail-first test that stays red has found a defect in merged,
approved code — park and report; the fix is its own bounded PR.**

---

## 8. Response to review — adopted, refined, pushed back

### 8.1 Adopted in full (verified against source first)

R1 gate design and its context-free rationale; R2's strike of the `:2957` atomicity argument and the
2c1/2c2 split; **R3 in full** — all 13 tests call legacy `configure_session` at `:2539`, `:2572`,
`:2607/2616`, `:2669/2683`, `:2869/2913`, `:3010`, `:3052`, `:3629`, `:3689`, `:3747`, `:3807/3815`,
`:3933`, `:4041/4050`, and **none** contains `FrozenCheckoutEffectV1` / `bound_spec` (those appear
only at `:1953-2160`), so the `WtEntry` discriminator keeps all 13 green and rev 1's inversion budget
and its R-6 mitigation regime are **deleted**; R4 (2a read-only, settlement to the writer, wire
contract, ten states); R5 (lock, replace primitive, routing, gate, custody-aware add, refusal
placement); R7 (2d re-scope); R9 (legacy-`Drop` ruling, my argument (c) struck); R10 (matrix
ownership); R11 (TDD structure, storage-report red test, 2c2 failure boundaries, peak measurement
dropped as a gate).

### 8.2 Adopted with refinement (evidence attached)

1. **`reap_idle`.** The review is right that my chain was wrong and right about the `:2257-2262`
   comment — but `reap_idle` **does** reach provider removal, via `:2311 claim.cleanup()` →
   `start_flight` `:328` → `:355-358`. My *conclusion* (idle reaping can delete a protected checkout)
   stands; only the route was wrong. 2b1's test surface uses the real chain.
2. **Eight deletion families.** Defensible as a taxonomy, but it conflates layers. Accurate structure:
   **3** `start_or_join_cleanup` callers → **7** in-crate families → **5** external subsystems, with
   `ExpiryClaim` alone contributing 12 call sites across three entry APIs plus `Drop`, and direct
   `release_session` exactly **11** non-test sites (the review's 11 is exact). The convergence is the
   *reason* one gate suffices, so the accurate structure strengthens R1.
3. **Cleanup single-flight (Sol 11).** The join key is `(session cell, strength)` — confirmed at
   `:555-571`. But **no preservation request exists today** and both strengths reach the same
   unconditional removal, so this is a **prospective** defect, not a current one. Recorded as a design
   requirement on 2c1 rather than a present bug.
4. **`decode_canonical` count.** Neither my 9 nor the review's 14: production is **13**, test 11,
   total 24. Two of mine were tests; I had missed five.

### 8.3 Pushback

1. **2b must be split — sizing.** Ruling R5 loads 2b with: replace primitive (~150), custody lock
   (~150), V3 routing across `admission.rs`/`executor.rs`/`execution_policy.rs`/`backend.rs` (~250),
   writer + ordering (~250), fail-closed gate (~120), custody-aware add across nine impls (~150),
   refusal (~60), storage report (~40), tests (~600) — **~1,770 LOC.** That exceeds one-round
   reviewability and trips the convergence rule I declared in rev 1 ("park and re-cut above ~1,800").
   I cut it into **2b1** (protection primitives + gate, ~700–850) and **2b2** (routing + writer,
   ~900–1,100). Every ruling's substance is preserved and the no-window discipline is *strengthened*:
   the gate now lands strictly **before** any V3 record can exist, rather than alongside the writer.
2. **The frozen-document amendment (R10) is outside my authorization.** The focused boundary is a
   checked-in frozen design record and my task is a scratchpad brief with no repo writes. I have put
   the ownership table in §6 as the authoritative assignment and written the exact amendment text so
   an authorized apply is mechanical — but I have not edited the frozen doc.
3. **R8's two options both rest on a premise 5.2 removes.** Neither cutover-in-2c1 nor a precursor PR
   is needed, because slice 2 has no production V3 write path at all. §7 item 5 already owns the
   terminal-row cutover by name. Recorded as a named dependency with sizing, not a slice-2 blocker.
