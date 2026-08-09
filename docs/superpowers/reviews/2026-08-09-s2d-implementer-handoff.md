# R2f1b slice 2d implementer handoff — claim exchange

## Status

The production-inactive claim-exchange mechanism is implemented. It has no `main.rs`, serving, executor, or resume-owner wiring; slice 5 remains the only owner of the production provider-effect half.

The first bridge verification reached Rust compilation and exposed the compile and Clippy defects corrected in this repair. The cached offline toolchain now passes workspace Clippy and all focused slice-2d regressions. The exact package gate has one known container-environmental `bridge-core --lib` errno-identity exclusion named in Gate output below; the task requires that defect to be parked rather than repaired in this slice.

## Delivered mechanism

`WorktreeCustodianV1::claim_exchange_for_successor` is a callable API in `custody_writer.rs`. It:

- validates both `WorkflowSnapshotV3` values, `validate_successor`, and each binding, returning the matched frozen checkout; it requires the supplied root, direct-child target, and retained source/root/worktree paths to equal that frozen object graph before it creates a custody lock or lease;
- preflights the canonical record under the pinned root before cells, requiring its worktree path to name that frozen target, then rechecks the full record under both cells to close the race;
- acquires the predecessor recovery lease in the shared namespace before entering cells. A held flock is a live predecessor and returns `Refused`; a free flock is held through successor acquisition;
- publishes only the frozen legal-table edge `LiveProtected -> RecoveredLive`, with `claim: null`. Under that edge no preserved claim exists, so `RecoveredLive.predecessor_claim_digest` is the canonical predecessor **snapshot** digest, not a claim digest;
- re-enters an exact stranded `RecoveredLive` record by acquiring only the missing successor lease: successor attempt, predecessor snapshot digest, custody identity, frozen target, and retained descriptor identities must all match, and this arm writes no record;
- stops after a classified ambiguous publication; after a durable replace it releases cells, acquires the successor lease, then releases the predecessor lease (the ordered lease transfer); and
- returns `ClaimExchangeReadyV1`, which owns that successor lease for the later slice-5 provider continuation. No provider is referenced by the mechanism.

## Obligation table

| Item | Implementation and regression coverage | Execution status |
|---|---|---|
| P1 | `successor_attempt_and_claim_exchange_validates_before_any_provider_call`; negative family for reused attempt, wrong origin, digest, lineage, and parent. The test-only continuation invokes the counting provider only after a ready token. | Focused regression passes |
| P2 | Frozen-table `LiveProtected -> RecoveredLive` replacement through `WorktreeCustodianV1`; terminal preservation stays byte-identical in `terminal_preserved_claim_is_not_exchanged`. | Focused regression passes |
| P3 | The row-6 regression asserts durable `RecoveredLive` state, gate refusal, and target retention. Its mutation discriminates via the **state assertion**, not a sweep assertion: both sweep arms are unconditionally non-destructive for every V3 record, a property pinned by slice 2a's unit test. | Focused regression passes |
| P4 | The successor lease is acquired only after durable publication; a post-predecessor-lease directory failure leaves `RecoveredLive` and returns `LeaseUnavailable`, which the row-6 repair test drives. | Focused regression passes |
| RA' | Live-predecessor refusal and successor-flock ownership are pinned by `a_live_predecessor_lease_refuses_without_writing_then_the_same_request_exchanges` and `successful_exchange_holds_the_successor_lease_until_the_ready_token_drops`. | Focused regression passes |
| RB' | The three `frozen_graph_…` tests reject a foreign root, a record naming another directory while the retained frozen identity stays intact, and swapped retained source/root before effects. | Focused regression passes |
| RC' | Row 6 repairs a stranded record without changing its bytes or inode; `recovered_live_reentry_refuses_a_different_successor_attempt` rejects mismatch. | Focused regression passes |
| P5 | First production-shaped `validate_successor` call is the custodian API; it is test-exercised only and has no production caller. | Focused regression passes |

## Design note 1 — API location

The API lives on `WorktreeCustodianV1`, rather than backend or `run_spec`. `bridge-worktree` already depends on `bridge-workflow`, so it can consume the frozen snapshots without creating an inverse dependency, and this layer owns the pinned root, both custody cells, canonical encoding, publication outcomes, and retained-identity checks. A slice-5 resume coordinator will obtain the frozen predecessor and successor snapshots plus bindings and retained identities, call this API, retain `ClaimExchangeReadyV1` while it resolves/configures its provider, then handle the provider result. That continuation is deliberately absent here.

## Design note 2 — crash-window ordering

1. **Validation:** invalid lineage, origin, digest, plan, node, frozen graph, retained identity, or record target returns `Refused`; no custody write, cell, or lease acquisition is possible.
2. **Liveness:** after validation and before cells, the predecessor recovery flock is acquired from the shared namespace. A held flock proves the predecessor live and refuses byte-identically; a free flock is held as the transfer source.
3. **Publication:** after the durable `RecoveredLive` replace and before successor lease acquisition, the record is recovery-owned: the sweep disposition is `Recover`, the backend presence gate refuses checkout removal, and the capability CAS refuses because it accepts only `LiveProtected`. `Ambiguous` stops here and writes nothing further.
4. **Lease:** cells are released after durable publication, then the successor lease is acquired before the predecessor lease is released. A crash before publication leaves `LiveProtected` and a process-released predecessor lease; a crash after publication but before successor acquisition leaves a re-enterable `RecoveredLive` record.

## TDD and mutation record

The original pre-implementation red-first observation was not retained, so it is not claimed retroactively. Five post-implementation mutation checks were run against the cached toolchain, each observed red and immediately reverted: P1 changed the frozen delivery-node comparison to `true`, making the `wrong-node` case fail; P2 bypassed both legal-edge guards, making `terminal_preserved_claim_is_not_exchanged` fail; P3 published `LiveProtected` rather than `RecoveredLive`, making the durable state assertion fail (not a sweep assertion: both V3 sweep arms are unconditionally non-destructive, as slice 2a pins); P4 forced the lease failure before publication, making the lease-window regression fail; and P5 removed `validate_successor`, making the wrong-digest case fail. The clean focused suite then passed all five tests.

## Repair round RA'–RD'

This declared adjudicated repair round adds no custody-table edge and leaves V2 plus the 13 legacy `configure_session` tests untouched.

- **RA' — predecessor liveness exclusion.** The exchange acquires the predecessor recovery lease in the same namespace as the successor lease before cells, retains it across publication, then releases it only after successor acquisition. The live-predecessor regression proves `Refused`, byte-identical record, no successor lease, no added custody locks, and success for the same request after release. The row-6 extension proves the returned token holds the successor flock.
- **RB' — frozen object graph.** Binding lookup returns the matched frozen checkout. The supplied root, direct-child target, retained source/root/worktree paths, and preflight record target are all bound before cells or leases; the locked read repeats the full record identity check.
- **RC' — stranded recovery re-entry.** Exact `RecoveredLive` re-entry requires successor attempt, predecessor snapshot digest, custody identity, frozen target, and retained descriptor re-verification. It acquires the missing successor lease without a `RecoveredLive -> RecoveredLive` publication.
- **RD'.** `ClaimExchangeOutcomeV1` is `#[must_use]`; the API documents blocking flock use and crash windows; `predecessor_claim_digest` is explicitly a predecessor snapshot digest; and the handoff/test-name corrections are recorded here.

### Repair red-first and mutation evidence

RA' red: holding the predecessor lease still returned `Exchanged` before the repair. After the fix, removing only the predecessor acquisition made `a_live_predecessor_lease_refuses_without_writing_then_the_same_request_exchanges` red; the mutation was reverted.

RB' red: a canonical valid `LiveProtected` record copied beneath a foreign root was exchanged before the repair. After the fix, disabling only root equality made `frozen_graph_wrong_root_refuses_before_locks_record_or_lease` red; the guard was restored. The direct-child check independently compares frozen target to frozen root so that mutation is discriminating. The corrected `frozen_graph_wrong_record_worktree_refuses_without_effects` regression leaves the retained frozen graph intact and makes only the record name an existing different directory. Disabling only its preflight record-target comparison creates the predecessor recovery lease before the locked full-record recheck refuses, so its zero-effects assertion is red; the guard was restored.

RC' red: a durable `RecoveredLive` record after successor-lease failure refused after repair. The re-entry fix made the row-6 repair test green. For the mutation, forcing the re-entry arm to atomically re-publish its identical record made the persistent-object identity assertion red (the bytes remain pinned byte-identically); the no-write arm was restored.

The publication fault seam is scoped to a particular `PinnedDirectoryV1` held by a custodian, while claim exchange creates that custodian privately. There is no existing route to arm an ambiguous replacement that landed for this API, so an ambiguous-then-retry fixture was not forced.

### Owner question — Candidate settlement

The brief §7 assigns the §6 “Candidate settlement” row wholly to 2d, but brief §3’s 2d steps omit it. `unused_candidate_settles_only_after_exact_absence` does not exist and `UnusedSettled` remains producerless under 2b2’s recovery-side ruling. Owner disposition is required; this repair adds neither a producer nor a transition.

## Slice-3 gate: §5.7 rows 1–6 and 12

| Row | Named regression | Status at this head |
|---|---|---|
| 1 | `custody_record_is_parent_synced_before_any_git_worktree_add` and `a_provider_without_custody_support_publishes_no_record_at_all` | Not confirmed: exact package gate blocked by unrelated bridge-core failures |
| 2 | staged-residue regression set, including `a_foreign_record_refuses_the_first_publication_and_the_temp_is_quarantined` | Not confirmed: exact package gate blocked by unrelated bridge-core failures |
| 3 | `protection_prepared_is_published_and_readable_before_any_provider_effect` | Not confirmed: exact package gate blocked by unrelated bridge-core failures |
| 4 | `partial_add_publishes_preservation_unknown_materialization_inflight` | Not confirmed: exact package gate blocked by unrelated bridge-core failures |
| 5 | `claim_renamed_with_ambiguous_parent_sync_stays_protective` | Not confirmed: exact package gate blocked by unrelated bridge-core failures |
| 6 | `claim_synced_but_lease_untransferred_keeps_both_protections` | Confirmed by focused regression; exact package gate blocked by unrelated bridge-core failures |
| 12 | `preserved_claim_awaits_r2f2_with_no_provider_replay` plus `terminal_preserved_claim_is_not_exchanged` | Not confirmed: exact package gate blocked by unrelated bridge-core failures |

Rows 1–5 and 12 are not reported green because the exact package gate stops in bridge-core before a trustworthy all-package result. Row 6 is green in its focused regression. The blocking core failures are outside this slice and must be resolved or dispositioned before slice 3 continues.

## Gate output

| Command | Result |
|---|---|
| `git diff --check` | PASS |
| `cargo fmt --all -- --check` | PASS |
| `env CARGO_HOME=/cargo CARGO_NET_OFFLINE=true cargo test -p bridge-worktree --test r2f1b_claim_exchange` | PASS: 12 passed |
| `env CARGO_HOME=/cargo CARGO_NET_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test -p bridge-core -p bridge-worktree -p bridge-coordinator -p bridge-controller -p bridge-workflow -p a2a-bridge` | EXCLUDED (exit 101 in the hermetic verifier): `bridge-core --lib` ran 477 passing tests and one known container-environmental failure, `fs_custody::tests::open_directory_no_follow_refuses_a_symlinked_directory`; it expected errno `ELOOP` (40) but observed `ENOTDIR` (20). The remaining package test binaries continued; no `bridge-core` source was changed. |

## §2c SELF-PASS

Claim checked: a claim exchange validates the successor against the frozen identity contract before any provider effect; a `RecoveredLive` record inherits every protection `LiveProtected` has — sweep-ineligible, gate-refused, mint-refused; no path this slice adds can remove, reset, clean, or prune a checkout; a terminal preserved claim that fails validation is never exchanged.

**Verdict: SURVIVED (focused dynamic regressions and mutations pass; exact package gate blocked by unrelated bridge-core failures).** Search scope: changed `custody_writer.rs`, `backend.rs`, and `r2f1b_claim_exchange.rs`, plus unchanged `sweep.rs`, `provider.rs`, `host_git.rs`, and `run_spec.rs`. The only new removal calls are test-fixture cleanup after assertions; the mechanism itself has no provider, reset, clean, prune, or removal call. The existing sweep handles `RecoveredLive` as recovery-only, and the gate keys on durable record presence.

## Remainder

Exactly the slice-2 brief remainder moves to slice 5: a durable V3 resume owner and the production half of “claim exchange precedes resume provider effect”, including serving parity. The Candidate-settlement gap is an owner disposition, not a producer added here. No new custody-table edge, V2 behavior, executor exit path, timer, resource-flight runner, deletion path, lock reclamation, or `workload_identity()` wiring was changed.
