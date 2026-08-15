---
task-type: implement
---
# R2f1b 3c2 Task E: API cleanup cell and exact checked-cleanup projection

## Description

Begin Task E on the exact accepted Task D head
`2697c43866db6a22549c9984f727fa911533b75d`. Implement the API-side cleanup
custody cell and the exact checked-cleanup projection over the existing
preserved 3c2 API backend. Production remains `LegacyV2` with
`resource_flight_route_v3 = None`; the old shared-flight request adapter
must still compile (Task F removes it); no HTTP execution paths migrate
here.

Own `crates/bridge-api/src/backend.rs`, focused API tests (colocated or the
crate's existing test layout), narrow `crates/bridge-api/src/lib.rs`
exports only if needed, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not modify bridge-core, the Task A-D surfaces, workflow/worktree crates,
or `bin/` production wiring.

Implement, per the binding salvage design:

- **the cleanup cell:** an `ApiRequestCleanupCustodianV1` cell keyed by the
  backend-global turn authority, installed in `SessionState` WHILE HOLDING
  the session lock, before Legacy or V3 admission leaves that lock; later
  bound to the exact request authority. Closed states:
  `AdmissionPendingLegacy | AdmissionPendingV3 | ActiveLegacy | ActiveV3 |
  DropOwned | Terminal | SettlementRefused | TimedOut`;
- **drop custody:** the request scope's drop first transfers the cell,
  settlement authority, acceptance state, observer, and the immutable
  cleanup deadline to the custodian — it never clears the slot after
  ignoring a result. The custodian may retry a refused LOCAL settlement
  only within the same request authority and never redispatches a provider
  effect. The durable request prefix remains the crash backstop;
- **bounded observation:** custodian observation is async
  (`tokio::sync::watch` or equivalent), deadline-bound, and leaves no
  blocking worker or OS thread after timeout;
- **exact checked projection** (the table is binding):
  no request/admission authority existed -> `Complete`; Legacy admission
  canceled with positive pre-send absence proof -> `Complete`; overlapping
  Legacy admission/request/drop/refusal/timeout -> `Unknown`; V3 canceled
  before the initial durable child with positive absence proof ->
  `Complete`; V3 terminal `Complete` plus matching publication
  acknowledgement -> `Complete`; V3 `Partial`/`Failed`/`Unknown`/pending
  publication/refusal/timeout -> `Unknown`;
- **surface coverage:** all four checked/observed forget/release cleanup
  surfaces use the cell; removing the session-map entry cannot remove
  cleanup debt; immediate same-ID session recreation uses a new
  backend-global authority and cannot touch the prior cell; void cleanup
  may discard the final disposition but performs the same custody transfer
  first; the backend records the exact `BackendCleanupDispositionV1` for
  `cleanup_cold_session` (Task G wires the retry consumer).

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility. A compile failure counts only when it is specifically the
  missing Task E API; zero selected tests does not.
- Active Legacy never claims `Complete` (red on the pre-change tree where
  the current projection collapses it).
- Cleanup landing in the bind/publication window projects `Unknown`.
- Terminal-refusal debt survives session-slot removal and is visible to a
  later checked cleanup.
- Drop retains acceptance-aware persistence diagnostics (the
  acceptance-barrier evidence is not lost when the scope drops).
- Proven completed work does not taint later independent cleanup of the
  same session.
- Forget-then-recreate: a stale authority for session A cannot signal,
  settle, or clean recreated B (extend the existing identity tests to the
  cell).
- Deadline expiry leaves zero live waiters and no blocking threads.
- All existing bridge-api tests keep passing unchanged (Wiremock suites
  included); the old request adapter still compiles with zero behavior
  change; production construction still assigns
  `resource_flight_route_v3 = None`.
- Run `cargo test -p bridge-api` (all harnesses) and
  `cargo test -p bridge-core --lib -- remote_request_flight` (unchanged),
  plus `git diff --check` and `cargo fmt --all -- --check`; no
  `rustfmt::skip`.
- Refresh the handoff: exact frozen input `2697c438`, red evidence, honest
  churn accounting (additions plus deletions, post-format), and the
  statement that Tasks F-G and production V3 remain unarmed.
- Stop and report a split before exceeding **500 changed production lines
  or 900 total changed lines** (churn convention) relative to `2697c438`.
  If the cap approaches, split the Legacy/cell foundation from the V3
  observation path and name E2 — do not touch HTTP execution either way.

## Files

- `crates/bridge-api/src/backend.rs`
- `crates/bridge-api/src/lib.rs` (narrow exports only if needed)
- focused API tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; earlier sections describe the preserved API backend and
  its cancellation/identity work)
- repository `AGENTS.md`

## Commit Message

feat(r2f1b): install the API cleanup cell and exact projection

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator will
first classify it: only a closed, enumerable rejection may receive one
targeted repair on this same artifact followed by one closure review. An
open-class or repeating family parks Task E. Never restart from a fresh
artifact and never silently extend the cap.
