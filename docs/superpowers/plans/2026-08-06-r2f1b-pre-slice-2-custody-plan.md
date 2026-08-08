# R2f1b pre-slice-2 gates and workspace-custody plan

**Status:** revision 3, owner decisions resolved 2026-08-07; implementation briefs authorized for Phase 0
(S1–S5) and Track A (A1–A5) as scoped below. This revision supersedes revision 2 (committed at `b133841`)
after the owner-side analysis in `~/Documents/R2f1b-custody-plan-rev2-analysis-fable.md` found five WRONG
findings and inverted priorities; §10 records the corrections. This document still does not authorize
automatic deadlines, provider turns, release, deployment, served-operator mutation, or any bridge-initiated
hosted-remote push (D-3 denies that authority).

## 1. Purpose

Close the pre-slice-2 gates mandated by the PR #50 closure record, and stop useful-work storage from
accumulating until the host runs out of space — with mechanism proportionate to the measured problem.
Measured decomposition (2026-08-06 cleanup + 2026-08-07 verification): 86.5% of recovered bytes were Cargo
targets, ~14% Docker volumes, 0% source; the largest surviving cost is 13.75 GiB of duplicated `.git`
object stores across 112 standalone implement clones. Therefore: build-target and clone hygiene first
(Phase 0), custody tests as mandated (Track A), and no remote-custody machinery unless a §8 trigger fires.

Two custody planes remain distinct (unchanged ruling):

1. **R2f1b runtime worktree custody** — workflow-created worktrees; durable record in the V3 workflow
   snapshot and custody sidecar.
2. **Implement-run workspace custody** — `.a2a-implement` quarantine clones; durable identity in fold
   receipts. Remote refs never enter `WorkflowSnapshotV3`.

## 2. Owner decisions — RESOLVED 2026-08-07

| ID | Decision | Ruling |
|---|---|---|
| **D-1** | Must pre-squash bot commits remain recoverable after squash-merge to main? | **No.** Content verifiably on `main` suffices; the clone reaper may delete. Fold receipts record `{run id, branch, pre-squash HEAD, tree, PR#, merge commit}` as the durable identity. This supersedes the ADR-0019/0026 keep-clones-for-operator ruling for merged/dispositioned runs. |
| **D-2** | Protected roots/resources | **Confirmed as listed:** user working checkouts; served operator + release/rollback artifacts; active bridge stores; live runs; current shared caches; every stockTrading/quant-platform repo, container, and volume. Reapers hard-refuse these regardless of classification. |
| **D-3** | May the bridge `git push` to a hosted remote on its own initiative? | **No.** Receipts only. No custody-ref namespace, no lease-bound push path, no bundles. Push authority stays with the owner's normal PR workflow. |
| **D-4** | Disk admission control | **One floor: 50 GiB.** Refuse to start a new full build/verify when free space on the data volume is below 50 GiB (single `df`-equivalent check, config-overridable). No watermark ladder, reservations, quotas, or caps. |

All sixteen revision-2 decision items are closed by the four rulings above, by defaults recorded in this
document, or removed as fake choices (orthogonal model, fingerprint domain, `fs_custody` owner,
feature-branch model — see §10).

## 3. Phase 0 — storage relief (five independent PRs)

Each slice is its own branch and PR, gated by `git diff --check`, `cargo fmt --all -- --check`,
warnings-denied all-target Clippy, full workspace tests, and `validate --repo-hygiene`, with a declared
**one-round review cap**. Findings ⇒ closed-enumerable targeted fix on the artifact; open-class ⇒ park and
escalate. Order S1→S5 is preferred but only S3/S4 depend on S2.

- **S1 — `CARGO_INCREMENTAL=0` for one-shot verification.** Set in the bridge's verify environment
  (container cache binding env alongside `CARGO_HOME`/`CARGO_TARGET_DIR`) and in CI one-shot builds; document
  the convention for native one-shot runs. Measured basis: 6.88 GiB of a 15.77 GiB verifier target (44%) was
  incremental artifacts. Note: `profile.release` already has `incremental=false`; this targets dev/test-profile
  one-shot builds. Interactive development keeps incremental compilation.
- **S2 — read-only `storage report`.** Walks bridge-owned roots only (`.a2a-implement`, worktree roots,
  per-repo cache volumes); emits per item: path, payload class (`SourceCheckout`, `BuildTarget`,
  `DependencyCache`, `Evidence`, `ContainerOrImage`), measured bytes, live-consumer status (lease, operation
  lock, process/open-file, container mount), git HEAD, and whether that HEAD is reachable from `origin`.
  No deletion, no push, no external effects. This is the audit instrument for S3/S4 and for §8 triggers.
- **S3 — build-target reaper + D-4 floor.** Deletes a completed run's build target once its gate output is
  reduced to retained evidence and no process, open file, or container owns it (rechecked at the destructive
  boundary; failed or nondiscriminating probes park the payload). Records logical size and physical reclaim
  separately. Implements the 50 GiB admission floor. D-2 roots are hard-refused.
- **S4 — clone reaper (enabled by D-1).** Reaps `.a2a-implement` clones whose useful content is verifiably
  on `main` (commit/tree containment check against the local source repo — no network required) or whose
  run the owner has dispositioned as abandoned. Writes the fold receipt before deletion. Dirty, untracked-
  nondisposable, submodule-dirty, or ambiguous clones are refused and reported, never deleted. Uses the
  exact-mechanism rules (guarded standalone-clone removal; canonical-path identity; never a broad prefix).
  Documents the `--no-hardlinks` quarantine tradeoff (the flag is load-bearing: hardlink-shared objects
  would let a `:rw` container corrupt the source repo's object store) and the D-1 ruling change.
- **S5 — same-repo verify serialization.** `verify.rs` claims same-repo verify runs are single-flight
  serialized; no lock implements it. Add an flock (reusing the ADR-0025 `PersistentLockGuard` primitives)
  keyed on the per-repo cache volume name, with a fail-first concurrent test. Correctness fix, independent
  of storage policy.

Expected recovery: the 36 GiB build-target class stops recurring (S1/S3); ~13.75 GiB of dead clone object
stores become reapable (S4); admission can no longer start a build the disk cannot hold (S3/D-4).

## 4. Track A — mandated pre-slice-2 gates, one PR per gate

The PR #50 closure record mandates before slice 2: close the remaining section-6 custody/flight/snapshot
tests and adjudicate the `fs_custody`/`local_file` extraction. Gate definitions are unchanged from
revision 2 §2 (recorded at `b133841`); they are referenced here, not restated.

- **A1 — runtime custody-test gate** (rev-2 §2.1): durable identity-bound custody before timers/effects;
  sweep survival; ambiguous evidence never deletes; global-outcome deletion authority; resume claim exchange.
- **A2 — preparation-flight (§2.2) + resource-flight (§2.3) test gates.** Split into two PRs if either
  review round finds a closed population larger than its cap.
- **A3 — V3 snapshot-test gate** (§2.4): complete unique custody plans; immutable delivery/contract bytes
  across successor resume; coherent lineage; manual-only V2.
- **A4 — `fs_custody`/`local_file` single-owner extraction** (§2.5): `local_file`'s duplicated pinned-
  directory/no-follow/sync/no-replace primitives become narrow wrappers over `fs_custody`; bounded-reader,
  quarantine, replacement, and compatibility-evidence policy stay in `local_file`. Parity + fault-injection
  tests. **A refactor — never rides in a test-only PR.**
- **A5 — workload-fingerprint binding** (§2.6). **Re-gated per the closure record to "before any
  `AutomaticR2f1b` construction"** — it may land after slice 2 begins and does not block slice-2 entry.

**Standing rule, declared before any dispatch:** a fail-first test that stays red has found a defect in
merged, approved code. **Park and report; the fix is its own bounded PR** with its own red/green control.
Test-landing PRs never silently expand into behavior fixes.

Roadmap step "reconcile and freeze main" is already done (`05db0fe`); the remaining reconciliation is the
closure record's stranded identities, handled as an addendum (§9).

## 5. Lifecycle model (simplified)

Revision 2's four-coordinate algebra is replaced by:

- **Phase:** the existing persisted `ImplementPhase` (`Cloned → EditStarted → FirstCommitCreated → InLoop →
  Approved | LoopStopped`) plus the merge outcome. No parallel state machine.
- **Payload class** (retained from rev 2 — its best idea): `SourceCheckout`, `BuildTarget`,
  `DependencyCache`, `Evidence`, `CredentialOrSecret`, `ContainerOrImage`. Cleanup rules attach to the
  class, never to the run directory as a prefix.
- **Durability**, recorded on the fold receipt at disposition time: `LocalOnly` | `OnMain{merge commit,
  tree}` | `Unknown`. Not a standing state machine; no materialization axis (a checkout is retained through
  review and repair — at 20–180 MiB it is noise next to a 20 GiB target; there is no eviction engine).

Cleanup admission per class (condensed from rev 2 §4, which remains the reference for S3/S4 briefs):

- **BuildTarget / DependencyCache:** exact path/type proven regenerable, not sole evidence, no live
  process/open-file/container owner, current shared cache preserved. `CACHEDIR.TAG` is evidence, not proof.
- **SourceCheckout (S4 only):** terminal run + cleaner-held operation lock + free lease; no scheduled
  consumer; no process/cwd/open-file/mount; git status (incl. untracked/ignored/submodule) matches
  disposition; content-on-main verified per D-1; receipt written; exact-mechanism removal; truthful
  partial/unknown recording.
- **Evidence:** own retention decision before any parent-directory deletion; never treated as cache.
- **CredentialOrSecret:** never pushed or bundled; retained/quarantined under its secret policy.
- **ContainerOrImage:** existing ADR-0021/0025 gates unchanged; current/rollback images protected.

Age and disk pressure trigger classification and priority. They never manufacture deletion authority.

## 6. Storage policy (simplified)

- **One admission floor (D-4): 50 GiB free**, checked before starting a full build/verify; refusal is a
  typed, actionable error naming the floor and the observed value. Config-overridable.
- **Reclaim order** when the floor (or the owner) triggers reaping: completed build targets → duplicated
  inactive dependency caches → D-1-eligible clones → zero-consumer containers/volumes under existing gates.
- **Protected roots (D-2)** are refused before classification, by canonical identity, never matched by
  prefix/age/size alone.
- **Measured baseline (2026-08-06, corrected):** data-volume free 328.93 → 371.00 GiB (observed gain
  42.07 GiB). Removed: 36.41 GiB host Cargo targets (component-measured deltas sum to 35.76 GiB; the
  0.65 GiB remainder is unattributed measurement drift) and 61.21 GB (57.0 GiB) *logical* Docker volume
  data yielding ~6.1 GiB *physical* (sparse OrbStack/APFS accounting). Components sum to ~42.5 GiB against
  42.07 GiB observed; the 0.4 GiB discrepancy is concurrent-activity noise, recorded rather than reconciled.
  2026-08-07 verification: 391 GiB free; `.a2a-implement` = 112 standalone clones / 16 GiB / 13.75 GiB
  duplicated `.git`. **Revision 2's "18–24 MiB source checkout" figure measured the eight *linked* sibling
  worktrees, not the governed clone population; it is corrected here (W1).**

## 7. Fold receipts (replaces the remote-custody plane)

At merge/disposition time the bridge writes one small JSON receipt per run (beside the existing checkpoint
evidence, surviving clone deletion): `{run id, task id, branch, pre-squash HEAD, tree, base, PR number if
known, merge commit, disposition, timestamp}`. Receipts are Evidence-class (never auto-deleted with the
clone). They are the answer to squash-stranding — which is real: the PR #50 closure record's own reviewed
tree `4fb6bfe` is locally unreachable (§9) — without any push authority. Durability of the content itself
is the owner's normal PR push of `main`.

## 8. Deferred machinery and triggers

| Deferred (from revision 2) | Build only if |
|---|---|
| Operator-invoked push command (`runs push <id>`) | Owner reverses D-1/D-3, or a class of parked-not-folded work accumulates that the owner wants durable off-host |
| Automated remote custody plane (lease-bound push, custody refs, CI namespace policy) | Multi-operator or multi-host operation |
| Git-bundle fallback + restoration proofs | A repository with no writable remote enters real use |
| Watermark ladder / reservations / per-repo caps / hot-storage budgets | Free space observed below ~300 GiB **with S1–S4 running** (storage report provides the evidence) |
| Hot leases / source pressure-eviction / exact reconstruction | Source-class footprint exceeds ~10 GiB after S4 (not expected) |
| Distribution profile experiment (`lto="thin"`, `strip="symbols"`) | Filed separately as release engineering; unrelated to custody. Rev-2's rejected defaults (no `opt-level=z`, no fat LTO, no `panic="abort"`, no `no_std`, no UPX) stand as conclusions without needing a decision. |

## 9. Reconciliations and deferral ledger

**Closure-record identity addendum (landed with this revision):** the closure record's "exact reviewed
aggregate tree `4fb6bfe1…`" and "integration commit `23ed6439…`" are not reachable in the local repository;
the bounded platform/coverage repair produced PR head `00f03c4` and the squash merge landed as `aedd2c2`
(tree `de53676`). The durable identity of the landed foundation is PR #50's merge commit. This is itself an
instance of the squash-stranding hazard §7 now guards against.

**Deferral ledger re-imported from the closure record** (dropped by revision 2; tracked here so the plan is
self-contained): cleanup-before-primary ordering; snapshot custody-plan coverage; legacy SQLite schema
rewrite; sequence/journal accounting; history-growth preflight symmetry; reserve literal binding; direct
`integrate_run_tree` tests; platform/test fixture edges; restoring `bridge-core` 86% → 90% and
`bridge-workflow` 87% → 90% coverage floors. Each is a bounded follow-on, none blocks slice-2 entry, and
none may be silently dropped from successor plans.

## 10. Corrections from revision 2 (audit trail)

Recorded so the lineage is reviewable; full analysis in the owner-side packet.

- **W1** — source-checkout pricing measured the wrong population (linked worktrees, not the 112 standalone
  `--no-hardlinks` clones); the 13.75 GiB duplicated-object-store class had no control. Fixed in §3 S4/§6.
- **W2** — source pressure-eviction could recover ≤~1.5 GiB against the 50 GiB cap that triggered it, and
  the reclaim order reached targets first; the lease/eviction/reconstruction subsystem was unreachable-by-
  need. Removed (§5, §8).
- **W3** — contradictory deletion authority for `Parked + BundleVerified` between §4.7 and §3/§6.3.
  Mooted: bundles removed (D-3, §8).
- **W4** — 2 workers × 25 GiB reservation exactly consumed the 50 GiB per-repo cap (and one 50 GiB slice
  reservation was 100% of it); effective concurrency 1. Removed with the quota scheduler (D-4).
- **W5** — the 6–8 checkout cap deadlocked admission against the existing 112 clones, which §4.7
  simultaneously refused to clean. Removed; S4 + D-1 clear the backlog instead.
- Plus: fingerprint re-gated to pre-`AutomaticR2f1b` (§4 A5); deferral ledger re-imported (§9); §6.1
  arithmetic/units corrected (§6); the sixteen-decision surface replaced by D-1..D-4 (§2); track monoliths
  replaced by per-PR slices with a red-test park-and-report rule (§3, §4).

## 11. Planning exit

Ready now: S1, S2, S5 (no dependencies), then S3, S4; A1–A4 in parallel with Phase 0 (A5 gated as stated).
Each brief inherits: exact base recorded at dispatch, the §3 gate set, a declared one-round review cap,
park-and-report on red gates or open-class findings, and roadmap + receipt reconciliation in the landing
commit. No further owner input is required for this scope; D-rulings above are the standing authority.
