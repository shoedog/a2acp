---
task-type: code-review
---
# R2f1b 3c2 aggregate review — concurrency and ownership lens (Sol/xhigh)

## Description

Perform the single aggregate concurrency-and-ownership review of the
COMPLETE 3c2 line: exact diff `42249b3d..50f3336e` in this checkout,
where `42249b3d` is the 3c1-folded main (the verified merge-base) and
`50f3336e` is the accepted final head of all eleven implementation
rounds (A1-A4, B1-B2, C, D, E, F, F2, G, G2). This is one completed
pass with no retry; a parallel independent lens reviews release,
compatibility, rollback, and cross-slice authority — do NOT spend your
pass there.

Your dimension: concurrency and ownership correctness across the whole
line — locks and lock order, atomics and fences, one-shot/linearizing
claims, absorbing states, drop/destructor custody, cross-thread
publication, journal/outbox atomicity, lease and authority lifetimes,
and eviction/retention of shared cells.

The line's major surfaces (all previously accepted through per-task
counted closures; every prior blocker was adjudicated FIXED at
per-task level — your job is the AGGREGATE view, interactions between
tasks, and the binding second looks below):

- `crates/bridge-core/src/fs_custody.rs` — journal-root binding/custody
  V2, debt-domination (`refuse_debt` first, census-before-refusal),
  SHA-256 staged-content commitment;
- `crates/bridge-core/src/namespace_transaction.rs` — replace/retire/
  recover with intent barriers, pre-removal commitment recheck,
  protective outcome lattice;
- `crates/bridge-core/src/remote_request_flight.rs` — request journal
  grammar, atomic admission (no zero-row), attempt lifetime lease
  (lease-before-operation-lock), full recovery table (pre-send
  `Failed,false`; `ProviderSendArmed` `Unknown,true`), idempotent
  publication outbox with exact delivery-identity acknowledgement,
  sealed non-cloneable authority, owned driver with first-poll arming
  fence, `failed_arm` zero-poll privilege, irreversible request-wide
  send permit, joinable publication flight;
- `crates/bridge-api/src/backend.rs` — the cleanup custodian cell
  (absorbing `TimedOut` across BOTH terminal writers), drop custody
  transfer with retained late flight, clone-record-then-clear
  diagnostic custody, request-local acceptance set only by the
  first-poll marker, honest publication acknowledgement (exact echo
  only), the migrated send path on the owned driver;
- `crates/bridge-workflow/src/executor.rs` — exact-disposition retry
  gating (`Ok(Unknown)` cannot redispatch at node or preflight
  consumers), preflight run-cache retention keyed on proven-clean,
  fallible metadata before effects;
- `crates/bridge-worktree/src/backend.rs` (tests) — the exhaustive
  two-field `CleanupReportV1` fold guard;
- `bin/a2a-bridge/src/smoke.rs` + `fallback_plan.rs` — typed protective
  release dispositions and the release-vocabulary reader.

BINDING second looks (each was folded as a disclosed operator
convergence extension after its counted round, with red controls; the
plan requires this aggregate pass to re-examine each at full depth):

1. **A4**: `fs_custody` publish derivation-after-census with `allowed`
   narrowing — confirm no census/derivation window admits a debted or
   reserved publish;
2. **B2**: reopen self-healing relabel-first ordering with the
   resumable intermediate — confirm a crash at any point in the heal
   leaves a resumable, never-corrupting state;
3. **C**: census-validation-before-recovery (`scan_with(op,
   tolerate_residue)` preflight in `open_base` before
   `NamespaceTransactionV2::recover`) — confirm no recovery path runs
   against an invalid row;
4. **G**: preflight fallible-metadata-before-effects — confirm no
   remaining pre-acceptance exit in `run_agent_preflight_uncached` can
   evict with backend effects behind it (the run-cancellation exits
   are exempt by the run-scoped-cache argument: a cancelled run has no
   later nodes).

Deferred-item adjudication (judge ONLY whether any hides an aggregate
blocker; they are otherwise ledgered test-hardening work): D's two
concurrency-test robustness defers (simultaneous-wrapper barrier test;
publication-waiter latch); E's two (admission-reset state-table
fail-first test; bound public-path stale-cell recreation test); F's
two (poll barrier around the real reqwest send; refusing/mismatched
publisher end-to-end cleanup tests); G's configure-clean eviction
regression. Known hermetic flake classes (flock-EBADF, 10/10
host-green; container signal-semantics; load-margin test hardenings
landed at `f04ec55e` and prescribed for the signal test) are
classified environmental — reopen one only with same-environment
evidence.

Sustained threat-model rulings are NOT to be reopened without a new
constructible WRONG: the cooperating-participant lease ruling (POSIX
has no inode-conditional namespace mutation; check-vs-syscall races on
cooperating participants land protectively), and the E admission-reset
relaxation (reset keys on terminal `Complete`; projection still
demands the acknowledgement; `TimedOut` never re-admits).

Supplied exact-head evidence is corroboration only; you are licensed
to falsify or reject every supplied result: head `50f3336e`, clean
worktree; host gates on exact `50f3336e` all exit 0 — full locked
all-feature workspace test **4,101 passed / 0 failed / 13 ignored
across 90 harnesses**, workspace clippy `-D warnings`, locked release
build, `cargo deny check`, hygiene. Production remains `LegacyV2` with
`resource_flight_route_v3 = None`; V3 is unarmed everywhere.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must
  name a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly report each of the four binding second looks as SUSTAINED
  or BROKEN with the mechanism.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-core/src/namespace_transaction.rs`
- `crates/bridge-core/src/remote_request_flight.rs`
- `crates/bridge-api/src/backend.rs`
- `crates/bridge-workflow/src/executor.rs`
- `crates/bridge-worktree/src/backend.rs`
- `bin/a2a-bridge/src/smoke.rs`
- `bin/a2a-bridge/src/fallback_plan.rs`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout — the complete per-task evidence trail)
- repository `AGENTS.md`
