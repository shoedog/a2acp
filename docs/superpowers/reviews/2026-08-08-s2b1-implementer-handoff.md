# R2f1b slice 2b1 — protection primitives + the fail-closed deletion gate — handoff

**Branch** `feat/r2f1b-2b1-protection-gate` in `/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s2b1`
**Base** local `main` @ `b4fc1ff3` (slice 2a folded). Commits:

| commit | scope |
|---|---|
| `cb1036d0` | D1 — `fs_custody` atomic replace + typed ambiguous outcome |
| `65eb1fe7` | D2 + D3 — custody lock module; `WtEntry` discriminator + the gate |
| `fbca3889` | R-11 external families (integration test crate + workflow collapse test) |
| `fb9aad76` | first handoff |
| *(this commit)* | repair round — R1 error-after-effect verification; R2 doc repairs + relocation; R3 deferred-ruling comment. A sha is deliberately not written here: it would be stale the moment this file is amended into the commit it names. |

**Review status.** Dual-lens adjudicated by the coordinator on primary evidence: opus SHIP, sol
REJECT / 3 BLOCKERs. One blocker (sol WRONG-3, error-after-effect) is repaired below as R1. Two are
DEFERRED with ledger obligations and are deliberately NOT attempted here — refusal-reported-as-clean
(2c1 owns the typed retained-disposition vocabulary) and the `Reserving` entry losing its cleanup
owner after a refused rollback (2c1 owns the state; re-inserting a refused reservation as `Ready`
would collide with the Ready-means-reusable hazard). Both are recorded in the gate's own comment so
they cannot be silently dropped — see §7.

---

## 1. What shipped

### D1 — atomic replace-and-parent-sync (`crates/bridge-core/src/fs_custody.rs`)

* `PinnedDirectoryV1::replace_regular_child(source, target_name, label) -> Result<ReplacePublicationV1, FsCustodyError>`
  — descriptor-relative, no-follow, single-component-validated, source-identity-checked, rename
  resolved against the retained parent descriptor.
* `rename_child_replacing(parent, source, target)` — the workspace's one replacing `renameat`.
  Separate function from `rename_child_no_replace`; plain POSIX `renameat` replaces atomically on
  every unix, so no `RENAME_*` flag and no per-OS arm.
* `ReplacePublicationV1 { Durable { retried_rename: Option<String> }, ParentSyncAmbiguous(String),
  TargetIdentityUnverified(String), RenameOutcomeUnverified(String) }`, `#[must_use]`, with
  `is_durable()` / `ambiguity()`. **The contract is the `Err`/`Ok` split**: `Err` means the rename
  provably did not happen (previous record intact and authoritative); every `Ok` arm means it did,
  except `RenameOutcomeUnverified`, which proves nothing in either direction. Nothing after the
  rename may be reported as `Err`, or a `?`-using caller would read "no effect" from an operation
  that had one. Only `Durable` is a clean success; every other arm is protective (§5.7 "Claim
  renamed, parent sync ambiguous"). `ambiguity()` is `Some` for every non-durable arm, so a later
  added arm is protective by default.
* **Error-after-effect verification (repair round, sol WRONG-3).** A failing `renameat` is not
  proof that nothing moved: on a network filesystem a retried RPC can perform the rename and then
  report a failure (server completed the first request, reply lost, retry finds the source gone).
  The original code mapped the errno straight to `Err` while the docs asserted universally that
  `Err` proves nothing moved — a narrow truth stated generally. Repaired by making the claim true
  rather than weakening it: `classify_failed_replace_rename` decides on descriptor-level identity,
  in evidence order — staged source still present AND still the caller's object ⇒ true `Err`;
  otherwise target is the caller's object ⇒ committed, continue the normal post-commit path with
  the syscall error retained in the outcome; otherwise ⇒ `RenameOutcomeUnverified`, protective.
  Rule 1 demands a POSITIVE identity match, not mere existence of the name, so a same-name
  substitution cannot be read as proof that nothing happened.
* `ReplaceRenameFaultV1 { BeforeEffect, AfterEffect, UnlinkSourceOnly }` +
  `PinnedDirectoryV1::fail_replace_rename_on_nth_call_for_test(call, shape)`. A new seam was
  required: an errno alone carries no information about what happened, so the seam has to state
  what the filesystem DID as well as what it reported. Compiled unconditionally, like the sync
  hook and for the same reason.
* `FsCustodyError::TargetMissing` — a replace asked to overwrite an absent record. Documented as
  **advisory**, not a linearization point (a replacing `renameat` has no "only if present" flag).
* `PinnedDirectoryV1::child_entry_exists(name, label)` — the no-follow "is this name taken" probe
  the gate needs. Answers `true` for directories, dangling symlinks and unreadable files: kind and
  readability are deliberately not consulted.
* **The no-replace paths are untouched**, pinned by
  `adding_the_replace_primitive_leaves_publication_no_replace`.

### D2 — custody lock (`crates/bridge-worktree/src/custody_lock.rs`, new)

A typed facade over the existing `liveness::acquire_persistent_lock_in` /
`acquire_persistent_lock_blocking_in` / `PersistentLockGuard`. No new locking mechanism. The five
required specifications, all in the module doc and all tested:

| | decision |
|---|---|
| (a) lock id | `custody_lock_id(&WorktreeCustodyIdV1)` — the id verbatim. `"custody-"` + 64 lowercase hex is a single path component by construction. Typed parameter so no caller can derive a key from a path or session id and miss the shared cell. |
| (b) directory | `custody_lock_dir(root)` = `<worktree root>/.custody-locks`. Same filesystem as the checkouts; dotted so **neither** sweep pattern selects it; deliberately never inside a checkout (a lock inside the directory whose deletion it guards is destroyed by that deletion). |
| (c) order | run lease → operation lock (implement-resume / verify / run-id) → **custody lock** → in-process backend mutexes. The custody lock is the innermost file lock; its holder must never take a run lease or operation lock. |
| (d) contention | `try_acquire_custody_lock_in` → `CustodyLockRefusalV1::Contended`. **Required for every deletion-side and sweep-side caller** — a cell a destructive caller cannot inspect is unknown, and unknown never licenses deletion. `acquire_custody_lock_blocking_in` waits, and is the transition writer's. |
| (e) guard owner | one custody transition (open → temp → sync → replace → parent-sync → reverify), released at its end. Not a lifetime lease. A destructive caller holds it across its whole decide-and-remove sequence. |

### D3 — `WtEntry` discriminator + one fail-closed refusal (`crates/bridge-worktree/src/backend.rs`)

* `WtCustodyV1 { Legacy, Protected }` on `WtEntry`. All **four** production construction sites set
  `Legacy`, so production behaviour is unchanged by the discriminator; the 13 legacy
  `configure_session` tests are untouched and green. Sites, by symbol (line numbers in this
  document are avoided for in-crate anchors — they drifted once already during the repair round):
  `configure_bound_resolved_with_admission` — its `reservation_entry` binding and its
  `WtState::Ready(WtEntry { .. })` publication; `configure_session` — the same twin pair.
* `checkout_removal_refusal(&WtEntry) -> Option<CheckoutRemovalRefusalV1>` — takes only a
  `WtEntry`, so it is **strength-independent and context-free by construction**: it cannot consult
  a `CleanupStrength`, a workflow outcome, or a cancellation cause even by accident.
* Called once in `run_cleanup_flight`, immediately before the `provider.remove` + sidecar removal
  sequence — find it by the comment banner `R2f1b fail-closed deletion gate (slice 2b1)`.
* `custody::probe_custody_record_presence(worktree_path) -> CustodyRecordPresenceV1
  { ProvablyAbsent, Present, Inconclusive(String) }` with
  `authorizes_checkout_removal()` — an exhaustive match mirroring 2a's
  `CustodySweepDispositionV1::authorizes_checkout_removal`.

**The discriminator/disk rule (documented on `checkout_removal_refusal`, pinned by tests).**
Durable truth is the record at `custody_record_path(worktree_path)`; the discriminator is an
in-memory cache of it. Removal is authorized only when **both** say unprotected, so the gate fails
closed in either direction of disagreement:

* discriminated but no record on disk → refuse (`the_discriminator_alone_refuses_deletion_with_no_record_on_disk`);
* legacy discriminator but a record present → refuse (every `publish_custody_record` test);
* probe cannot answer → refuse (`an_unreadable_custody_probe_refuses_deletion`).

The disk arm is what survives a process restart, which the discriminator alone cannot: after a
crash the map is empty and every rebuilt entry is `Legacy`. The disk arm keys on record
**presence**, never on a successful decode, so protection cannot be removed by damaging the record
(`a_corrupt_custody_record_still_refuses_deletion`).

**V2 BEHAVIOUR CHANGE — the one this slice does introduce (opus S-5).** The discriminator alone is
behaviour-neutral, but the disk arm is not: **every V2 cleanup now probes the filesystem** before
`provider.remove`. `probe_custody_record_presence` pins the checkout's enclosing directory and stats
one name, so a transient read failure on the worktree ROOT that leaves it existing-but-unpinnable
(EACCES, a mount transition, ENOTDIR) resolves to `Inconclusive` and the gate **refuses an ordinary
V2 removal**. The consequences, stated plainly:

* it is a protective leak, not data loss — the checkout and its sidecar stay on disk;
* it is reported to the caller as a **clean cleanup** (refusal-as-`Ok`), so nothing observes it;
* it is self-healing for V2, because the retained `.meta.json` keeps the checkout selectable by the
  legacy sweep arms — `WorktreeRunEndGuard::drop` reclaims it at clean run exit, and `sweep_orphans`
  reclaims it on the next boot after a crash. (This is the same retention that OPEN-2 above turns
  into a hazard once V3 records exist; for V2 with no custody record it is the recovery path.)

A permanently unpinnable root would leak permanently, but that state also means no worktree can be
created there, so it is not a silent-degradation risk. Not gated on, and no test asserts the leak is
bounded — `an_unreadable_custody_probe_refuses_deletion` pins the refusal, not its recovery.

**Three design decisions inside the gate, each with a stated reason:**

1. **Refusal reports `Ok`, not `Err`.** It is not a cleanup failure — the inner session teardown
   completed and only the checkout was retained. And an `Err` would be fatal: the failed-configure
   reporter loop retries on an `Err` report while `failed_configure_cleanup_pending` is set, so a
   protected rollback would spin forever at the 30 s backoff cap.
2. **A genuine inner-teardown failure is still surfaced.** The early return re-raises the
   accumulated `first_error`; pinned by `a_refused_checkout_still_reports_a_failed_inner_teardown`.
3. **In-memory state stays consistent.** `provider_removed` / `sidecar_removed` stay false and
   `state.entry` stays populated, so the map is never emptied as if the checkout were gone and any
   later flight re-runs the same refusal. Cleanup cells are per-session, so a refusal cannot wedge
   another session — pinned by
   `retire_refuses_a_protected_checkout_and_still_drains_an_unprotected_sibling`.
   *Deferred (2c1), and recorded in the gate's own comment:* for a `Reserving` entry,
   `entry_for_cleanup` pops the map entry *before* the gate runs (pre-existing behaviour, releasing
   the reservation so a configure can retry), so a refused rollback LOSES its cleanup owner — the
   entry survives only in `state.entry` and a later `configure_session` starts a fresh reservation.
   2c1 owns the state model; re-inserting a refused reservation as `Ready` here would collide with
   the Ready-means-reusable hazard. **2b2 must land the `cleanup_failed_add` prohibition in the same
   PR as the V3 writer**: without it, a refused rollback followed by a configure retry reaches
   `HostGitWorktree::add`'s `cleanup_failed_add`, whose `remove_dir_all` is outside this gate.

   *Also deferred (2c1):* refusal-as-`Ok` is the 2b1 **interim projection**. Until 2c1 mints the
   typed retained/refused cleanup disposition, an observer cannot distinguish "cleaned" from
   "deliberately retained".

---

## 2. R-11 coverage

Where families share one code path, the collapse is stated and was verified in source (citations in
each test's docstring), not assumed.

| R-11 family | test | path-collapse justification |
|---|---|---|
| `ConfigureAdmission::Drop` (`impl Drop for ConfigureAdmission`) | `configure_admission_drop_cleanup_refuses_to_delete_a_protected_checkout` | direct — the `Drop` impl's own `start_or_join_cleanup(Release, true)` |
| bound-configure rollback | `bound_configure_rollback_refuses_to_delete_a_protected_checkout` | all six arms call `cleanup_session_with_sealed_admission(Release, true)`; the provider-add arm is driven |
| legacy-configure rollback | `legacy_configure_rollback_refuses_to_delete_a_protected_checkout` | same, seven arms; the inner-configure arm is driven |
| `retire()` (`WorktreeBackend::retire`) | `retire_refuses_a_protected_checkout_and_still_drains_an_unprotected_sibling` | direct |
| forget (`forget_session` / `forget_session_checked`) | `forget_refuses_to_delete_a_checkout_that_has_a_custody_record` | direct; also the `Forget` half of strength-independence |
| release (`release_session` / `release_session_checked`) | `release_refuses_to_delete_a_checkout_that_has_a_custody_record` | direct; the `Release` half of strength-independence |
| observed cleanup (`forget_session_observed` / `release_session_observed`) | `observed_cleanup_refuses_to_delete_a_checkout_that_has_a_custody_record` | direct, both strengths |
| `BindingGuard::Drop` (`dispatch.rs:48-64`) | `binding_guard_drop_cannot_delete_a_custody_protected_checkout` (integration) | direct — real guard, real `Drop`, waits for the spawned eviction to reach the backend |
| `ExpiryClaim` — `cleanup()` | `idle_reaping_cannot_delete_a_custody_protected_checkout` (integration) | the real chain `reap_idle:2312` → `cleanup:409` → `start_flight:328` → `release_session_checked:356-359` |
| `ExpiryClaim` — `into_flight()`, `Drop` | (same test) | `into_flight` (`session_manager.rs:393-395`) and `Drop` (`:420-423`) are one-line wrappers over the same `start_flight`, whose only backend call is `release_session_checked` — read in source |
| 11 direct `release_session` sites | `a_direct_release_session_cannot_delete_a_custody_protected_checkout` (integration) | all eleven (`session_manager.rs:1101, 1206, 1224, 1352, 2015, 2025, 2036, 2162, 2170, 2181, 2228`) are `backend.release_session(&session).await` on `Arc<dyn AgentBackend>` — the method the test invokes |
| workflow cold cleanup (`executor.rs:966-987`) | `cold_cleanup_reaches_the_backend_only_through_the_observed_methods` (bridge-workflow) + `observed_cleanup_refuses_...` | the workflow test pins that `cleanup_cold_session` touches the backend through **only** `forget_session_observed`/`release_session_observed`, and records `release_session`/`forget_session`/`retire` separately so a later bypass fails the test |
| controller retire (`resilient.rs:69-72`, `:178`) | `controller_retire_cannot_delete_a_custody_protected_checkout` (integration) | driven through the real `ResilientWarm`; both sites call `AgentBackend::retire` |

**Fail-closed variants.** `the_discriminator_alone_refuses_deletion_with_no_record_on_disk`,
`a_corrupt_custody_record_still_refuses_deletion`, `an_unreadable_custody_probe_refuses_deletion`,
`a_refused_checkout_re_refuses_and_becomes_deletable_once_its_record_is_gone`,
`a_refused_checkout_still_reports_a_failed_inner_teardown`.

**Positive controls** (the gate must narrow deletion, not neuter it):
`an_unprotected_checkout_is_still_deleted_by_every_cleanup_strength` (forget + release),
`idle_reaping_still_deletes_an_unprotected_checkout` (the real reap chain),
`retire_refuses_a_protected_checkout_and_still_drains_an_unprotected_sibling` (retire), plus the
131-test `bridge-worktree` suite including the 13 untouched legacy `configure_session` tests.

**Mutation evidence.** With `checkout_removal_refusal` forced to `None`, **all 12** new in-crate
gate tests and **all 4** protective integration tests fail; both positive controls still pass. Run
and recorded during development; not a checked-in artifact.

---

## 3. Gates (re-run in this worktree after the repair round)

| gate | result |
|---|---|
| `git diff --check` | exit 0, no output |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check --workspace` | exit 0, zero warnings |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0, zero warnings |
| `cargo test -p bridge-core -p bridge-worktree -p bridge-coordinator -p bridge-controller -p bridge-workflow` | **1248 passed, 0 failed, 0 ignored**, 24 suites |

Per-suite totals for the five focused packages (`+3` in `bridge_core` vs the pre-repair run — the
three R1 injection tests):

```
bridge_controller  lib                          141 passed
bridge_coordinator lib                          273 passed
bridge_core        lib                          468 passed   (+3)
bridge_core        tests/compile_fail             1 passed
bridge_core        tests/r2b1_diagnostics        21 passed
bridge_core        tests/r2b2_observers           5 passed
bridge_core        tests/r2b2_warm_survivability  4 passed
bridge_core        tests/r2f0b_contract          11 passed
bridge_core        tests/r2f1a_execution_policy  14 passed
bridge_workflow    lib                          124 passed
bridge_workflow    tests/r2f1a_admission          5 passed
bridge_workflow    tests/r2f1a_bound_executor    13 passed
bridge_workflow    tests/r2f1a_fanout             3 passed
bridge_workflow    tests/r2f1a_run_spec           3 passed
bridge_workflow    tests/r2f1b_run_spec_v3       14 passed
bridge_workflow    tests/r2f1b_workload_identity 10 passed
bridge_workflow    tests/workflow_context_compat  1 passed
bridge_worktree    lib                          131 passed
bridge_worktree    tests/r2f1b_deletion_gate      5 passed
doc-tests (5 crates)                              1 passed
```

**Not run, and why:** `cargo test --workspace`, `cargo build --release --bin a2a-bridge`, and
`cargo run -p a2a-bridge -- validate --repo-hygiene` — the dispatch brief assigns the full workspace
suite and the release/hygiene gates to the orchestrator at fold time.

**Platform exclusion (standing R-10).** Ran on darwin (macOS) only. Every new `fs_custody` test, the
whole replace primitive, the error-after-effect verification, and the dev/ino arms of the presence
probe are `#[cfg(unix)]`; the non-unix arms return `Unsupported`/`Inconclusive` and are unexercised.
The Linux `renameat2`/`RENAME_NOREPLACE` arm of `rename_child_no_replace` and the Linux path of
`rename_child_replacing` were **not** executed — only the macOS `renameatx_np`/`renameat` arms.
Nothing here was run against a real network filesystem: the error-after-effect behaviour is
exercised through the injected `ReplaceRenameFaultV1` seam, which models the NFS retried-RPC shape
rather than reproducing it.

**Mutation evidence for R1** (run during the repair, not a checked-in artifact): reverting
`classify_failed_replace_rename` to the naive errno mapping fails
`a_rename_that_took_effect_despite_a_syscall_error_is_never_a_plain_error` and
`a_rename_error_whose_effect_cannot_be_verified_is_protective` while the `Err` control still passes;
the opposite mutation (never return `Err`) fails
`a_rename_error_with_no_effect_is_still_a_provably_not_renamed_error`. Both directions discriminate.

**Size.** Total diff **+2,594 / −1** across 10 files (of which 389 lines are this document). Code
split: **~857 lines of non-test code** — a large share doc comment, per the 2a house style — and
**~1,348 lines of test**. Against the brief's ~700–850 LOC estimate this is ~3x; the overrun is
test surface (R-11 has twelve families, each required to be proved) plus the repair round. Scope was
not grown: the deliverables are exactly D1/D2/D3. Flagging rather than deciding — the production
diff is one-round reviewable and the tests are one-per-family.

---

## 4. Parked findings

**One parked defect in merged code** (found by this round's repair, not fixable inside it), plus two
carried observations with named owners.

### PARKED-1 — `rename_child_no_replace` has the identical error-after-effect hazard (merged A4)

The R1 repair below fixes the REPLACE path. Its sibling is unrepaired: `rename_child_no_replace`
(`crates/bridge-core/src/fs_custody.rs`) is called by `publish_new_regular_child_impl`, which maps
the errno straight to `Err(FsCustodyError::Io)` with no verification. Same mechanism, same
filesystems: a retried NFS RENAME RPC can create the target and then report `ENOENT`, so a caller
that believes the publication failed is wrong.

**Direction of the failure is fail-OPEN, which is why it matters.** For a `ProtectionPrepared`
publication, "publish reported failure but the record exists" means the writer concludes the
checkout is unprotected while the on-disk record says otherwise. 2b1's deletion gate happens to
catch that specific case (its disk arm refuses on record presence regardless of what the writer
believed), but nothing else does, and the writer's own control flow — abandon, retry, quarantine —
is driven by the wrong answer.

Not repaired here per the standing rule (custody plan §4): a defect found in merged, approved code
is its own bounded PR. The repair is mechanical — the same `classify_failed_*` shape, with rule 2
reading "target exists AND is our object ⇒ committed". A cross-reference is left in
`rename_child_replacing`'s doc comment so the two cannot drift apart silently.

### OPEN-2 (2b2 obligation) — a checkout carrying BOTH records is deleted by the legacy sweep arms

**Correcting the first version of this handoff, which was wrong.** It claimed 2a's
`legacy_boot_arm_still_reclaims_alongside_a_v3_record` pins the coexistence case. It does not:
that test writes `write_worktree_sidecar(&root, "dead", …)` and
`write_custody_checkout(&root, "v3", …)` — **two separate checkouts**. It proves only that a V3
record *elsewhere in the root* does not stop the legacy arm reclaiming a different dead checkout.
Nothing pins one checkout carrying both a `.meta.json` and a `.custody.v1.json`.

And that state is exactly what **this slice's gate produces**. On refusal the flight returns before
the sidecar removal, so `state.sidecar_removed` stays false and the legacy `<target>.meta.json`
remains beside the custody record. Both legacy sweep arms then select it by its sidecar and delete
the checkout:

* `sweep_orphans`'s legacy arm — reads the `.meta.json`, classifies by lease, `Verdict::Dead` →
  `remove_worktree_if_safe`; the custody record is never consulted for a legacy-scanned entry.
* **`WorktreeRunEndGuard::drop`'s clean-drop arm** — the sharper of the two, and it was missing from
  the first version. It matches on `s.run_id == self.instance_id` and calls
  `remove_worktree_if_safe` at the end of every *normal* bridge run. A checkout refused by the gate
  during a run and still carrying its sidecar is deleted at run end by the same process, with no
  crash and no boot sweep involved.

Not a live hazard in 2b1 (nothing writes a custody record in production yet), so it is an **OPEN
2b2 obligation**, not a parked defect. Suggested test name:
`a_checkout_carrying_both_records_is_reclaimed_by_neither_sweep_arm`. Note the obligation has two
halves: the V3 writer must not emit `.meta.json` (`v3_path_writes_no_legacy_meta_json`), **and** the
sweep arms must refuse a checkout whose custody record is present regardless of a sidecar, because
the refusal path above can create the coexistence state without any writer emitting both.

### OBS-3 — `implement::reset_worktree_to_head`

`bridge-controller/src/implement.rs:511-515` runs `git reset --hard HEAD` + `git clean -fdq`,
production-wired through `ResilientWarm`'s transient respawn (`resilient.rs:180`; constructed at
`main.rs:3423`, `:3775`). §5.1 forbids reset/clean for a protected checkout and §5.4 already rules
that path "must not be reachable for an R2f1b-protected attempt". It destroys work **in place**
without removing a checkout, and it targets the implement clone rather than a `[worktrees]`
checkout, so it does not refute the removal claim — but it is destructive, ungated, and belongs to
whichever slice implements §5.4's `ResilientWarm` obligation.

---

## 5. §2c SELF-PASS — **REFUTED** (adversarial, NOT independent)

**Claim under test.** "Every production deletion path that can remove a worktree checkout funnels
through the removal block in `run_cleanup_flight`, so the single gate covers all R-11 families."

**Search scope.** Whole repo, `crates/` + `bin/`, `--include="*.rs"`, both production and test files
with test modules identified by the first `#[cfg(test)]` marker. Patterns: `remove_dir_all`,
`remove_dir(`, `remove_file`, `remove_argv`, `prune_argv`, `"worktree"` + `remove|prune`,
`provider.remove` / `.remove(&entry`, `reset --hard`, `"clean"`, `verify_then_remove`,
`ItemSource::WorktreePath`, `sweep_orphans`, `WorktreeRunEndGuard`, `ResetWorktree`, `ResilientWarm`.
Also read in full: `WorktreeProvider` (all argv builders), `host_git.rs`, `sweep.rs`,
`storage_reap.rs`'s source discrimination, and both `ResilientWarm` production construction sites.

**Verdict: REFUTED as stated.** Three production sites remove a worktree checkout; only one is the
gated block.

| # | site | reaches `run_cleanup_flight`? | owner |
|---|---|---|---|
| 1 | `provider.remove` + sidecar removal inside `WorktreeBackend::run_cleanup_flight` | **yes** — this IS the block | gated by 2b1 |
| 2 | `sweep::remove_worktree` (`sweep.rs:23-31`: `git worktree remove --force`, `worktree prune`, `remove_dir_all`, `remove_file`), reached from `sweep_orphans` (`:242`, installed `main.rs:3368/3723/4344/7899/9584`) **and** `WorktreeRunEndGuard::drop` (`:304`, installed `main.rs:3389/3739/4352`) | **no** | 2a — the V3 arms are non-destructive; the legacy arm stays destructive by the R-4 ruling |
| 3 | `host_git::cleanup_failed_add` (`host_git.rs:42-47`, `remove_dir_all(wt)`), called at `:137` and `:147` inside `HostGitWorktree::add` | **no** | 2b2 — this is R-7 verbatim, and it is currently **ungated** |

**Correction, left visible.** The narrower claim the slice actually rests on **SURVIVED**, verified
family by family: *all twelve R-11 families funnel through the one removal block, so the single gate
covers all R-11 families.* The in-crate structure is exactly as the brief states — three
`start_or_join_cleanup` callers (`ConfigureAdmission::drop`, `cleanup_session_with_sealed_admission`,
`cleanup_session_observed`), two `run_cleanup_flight` spawn sites (the initial spawn in
`start_or_join_cleanup` and its failed-configure re-spawn in the reporter task), and every
`cleanup_session*` variant funnels into those three. The five
external subsystems reach it through `AgentBackend` cleanup methods only, each pinned by a test
above.

What the refutation costs: the *stated* claim overreaches, and a reviewer or downstream slice that
took it literally would conclude 2b1 closes every checkout-deletion path. It does not. Sites 2 and 3
are real production deletion paths outside the gate. Site 2 is already handled for V3 by 2a. **Site
3 is not handled at all today** — nothing prevents `HostGitWorktree::add` from `remove_dir_all`-ing a
directory that already carries a custody record. That is harmless in 2b1 (no production path writes
one) and R-7 assigns it to 2b2, but it must not be dropped: after 2b2's writer lands and before its
add-prohibition, a partial add would delete a protected checkout.

Also checked and cleared (destructive, but not checkout removal): `merge.rs:605`'s
`remove_dir_all(&cclone)` reaps an implement **clone** behind four guards, never a `[worktrees]`
checkout; `storage_reap.rs:1114` refuses `ItemSource::WorktreePath` outright before any
classification; `compatibility.rs:2309`'s `ScratchDir::drop` clears a compat scratch directory;
`fs_custody::verify_then_remove` is called only by the two storage reapers, which the previous point
excludes from worktree paths.

---

## 6. Deliberately NOT done

* **No V3 record writer, no routing/propagation, no creation-ordering inversion** (2b2). The replace
  primitive and the lock exist and are tested; nothing calls them from a production path yet.
* **No preservation transitions, no barrier placement, no single-flight key extension** (2c1).
* **No `DeleteAuthorized` CAS, `DeletionCapabilityV1`, `remove_v2`, or post-loop mint** (2c2).
* **No claim exchange** (2d).
* **No change to `WorktreeProvider::add` or `cleanup_failed_add`** (2b2 / R-7) — see the §2c
  correction, this is the one uncovered production removal path.
* **No `workload_identity()` wiring** (slice 4); no `NodeCleanupV2` tightening.
* **The 13 legacy `configure_session` tests are untouched**, as required by brief §8.1 R3. The
  discriminator addition is behaviour-neutral for them.
* **`sweep.rs` untouched** — the redundant-guard coverage item in
  `end_guard_skips_sidecar_that_points_at_non_sibling_worktree`'s docstring stays 2b2's.
* **The custody lock is not wired to anything.** No transition, sweep, or cleanup path acquires it
  yet; 2b1 ships the primitive plus its five-part contract, and the acquisition-order declaration is
  a claim about future callers, not an enforced invariant. A reviewer should treat the ordering
  section of `custody_lock.rs` as a specification to check 2b2/2c against.
* **The gate does not lock.** `checkout_removal_refusal` reads the record's presence without holding
  the custody cell, so a transition could in principle publish between the probe and
  `provider.remove`. That is deliberate for 2b1: nothing publishes records in production yet, and
  the correct fix (hold the cell across decide-and-remove) belongs with the slice that creates a
  concurrent writer. Flagging it explicitly so it is a decision, not an omission.
* **No `#[cfg(not(unix))]` behaviour was exercised**, and no Linux run happened — see the gate
  section's platform note.
* **The two deferred review blockers were not attempted** (§7), on the coordinator's adjudication.
* **`rename_child_no_replace` was not repaired** despite carrying the identical error-after-effect
  hazard the R1 round fixed on the replace path — merged A4 code, parked per the standing rule
  (PARKED-1 in §4). Only a doc cross-reference was added.

---

## 7. Deferred review blockers — ledger obligations

Two of sol's three BLOCKERs were adjudicated DEFERRED by the coordinator and are deliberately not
attempted in this slice. Both are recorded in the deletion gate's own comment in `backend.rs` so
they travel with the code, not only with this document.

| # | finding | why deferred | owner + obligation |
|---|---|---|---|
| D-1 | A refusal is reported to the fan-in as a clean cleanup, so no caller can distinguish "removed" from "deliberately retained". | The typed retained/refused cleanup disposition — and ownership retention through a refusal — is vocabulary 2c1 mints. Inventing a local one in 2b1 would have to be re-cut. Returning `Err` instead is not available: the failed-configure reporter retries on `Err` while `failed_configure_cleanup_pending` is set, so a protected rollback would spin at the 30 s backoff cap forever. | **2c1** — mint the disposition and re-project the gate's refusal onto it. |
| D-2 | A refused rollback of a `Reserving` entry loses its cleanup owner: `entry_for_cleanup` pops the map entry before the gate runs, so a later `configure_session` starts a fresh reservation over a protected checkout. | 2c1 owns the state model. Re-inserting a refused reservation as `Ready` here would collide with the Ready-means-reusable hazard. | **2c1** — state model. **2b2 — hard prerequisite:** land the `cleanup_failed_add` prohibition in the SAME PR as the V3 writer. Without it, refused rollback → configure retry → `HostGitWorktree::add` → `cleanup_failed_add`'s `remove_dir_all` on the protected checkout, entirely outside this gate. |

Cross-reference: D-2's `cleanup_failed_add` exposure is the same site as row 3 of the §5 SELF-PASS
table — the one production checkout-removal path that is ungated today.
