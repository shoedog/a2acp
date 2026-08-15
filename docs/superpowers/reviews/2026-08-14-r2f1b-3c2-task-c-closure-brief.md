---
task-type: code-review
---
# R2f1b 3c2 Task C closure review

## Description

Perform the one declared hard-read-only closure review of the complete Task
C line: exact diff `dbf514bd..8e50669` in this checkout, where `dbf514bd`
is the accepted Task B head and `8e50669` is the current head. Review the
full diff and the complete module plus the one authorized custody accessor
in context. Do not edit, build, test, invoke another provider, or access
the network. This review round is capped at one; no repair loop is
authorized inside it.

The line contains two commits:

1. `4db414f0` — the Task C implementation (480 production / 845 total
   churn, in caps): the durable send-state progression
   (`Reserved -> IntentJournaled -> DispatchAuthorized -> ProviderSendArmed
   -> TerminalPendingPublication -> PublicationAcknowledged`) recorded
   through the Task A surfaces with only exact `Complete` advancing; the
   attempt lifetime lease with `open_recovered` as the only
   production-capable constructor and typed `AttemptLive` refusal; the
   complete recovery table (pre-send prefixes recover `Failed` with
   `accepted = false`; `ProviderSendArmed` recovers `Unknown` with
   `accepted = true`; `TerminalPendingPublication` replays the durable CAS
   winner idempotently; `PublicationAcknowledged` retires without
   republishing; invalid order/identity/digest/schema refuses the whole
   attempt byte-preserved); the idempotent publication outbox
   (`publish_idempotent`, exact delivery-identity echo, pending outbox
   blocks admission, no no-op sink); and the binding authority rider
   (attempt + ordinal bound privately into the authority and every
   delivery/control key). Its advisory review confirmed the design but
   found two blockers.
2. `8e50669` — the one declared targeted repair (fs_custody +102, module
   +140/−50, handoff +79; advisory review APPROVE with two low-risk
   DEFERs): admission and reopen headroom now count the permanent lease
   child (effective footprint four) with the interrupted positive-edge
   admission healable on reopen and the cap-edge tests migrated red-first;
   and `open_recovered` acquires the lifetime lease BEFORE any Task A
   operation, via one operator-authorized narrow `pub(crate)` custody
   accessor (`fs_custody`) that opens an existing regular child no-create/
   no-follow with identity verification and nonblocking flock — no
   mutation authority, no path projection, colocated refusal/contention
   tests. A holder elsewhere now yields exact `AttemptLive` with zero
   mutation; a lock-order regression proves lease-before-operation.

Operator adjudications you must independently judge:

1. The narrow fs_custody accessor was operator-authorized as a Task A
   surface ADDITION consistent with the custody model (the design's total
   lock order places the lifetime lease before the operation lock; the
   accessor grants open+flock on a pre-existing child only). Judge whether
   it weakens any Task A guarantee (creation, mutation, path authority,
   residue classification).
2. The below-checkpoint ambiguity that B2 deferred is resolved by the
   send-state rows: judge that reopen/recovery now distinguishes admitted,
   armed, terminal, and acknowledged children truthfully, and that the
   pre-send `Failed`/`accepted=false` claim never covers a child whose row
   reached `ProviderSendArmed`.
3. The in-container verify was fully green on both C runs; the lane's
   flock-EBADF hermetic class did not recur here.

Required judgments:

- Lease correctness: second live opener (same-process second handle and
  cross-lease contention) yields exact `AttemptLive` with unchanged bytes;
  the lease releases on drop; reopen succeeds afterward; no path acquires
  a Task A operation before the lease.
- Recovery-table completeness and idempotence for every durable prefix,
  including replay-after-crash publisher calls deduplicated by delivery
  identity at the contract level.
- Outbox discipline: pending publication blocks admission; mismatched and
  refused acknowledgements are distinguished exactly; sink-commit-before-
  ack ordering holds.
- Authority binding: identical request ids across two attempt roots cannot
  alias authorities, publications, or controls.
- Capacity: the lease is counted everywhere (admission, reopen, headroom
  for checkpoint replacement); the maximum-occupancy admission refuses
  before mint; the positive edge admits and completes.
- Scope: only the owned module, the one authorized accessor, exports, and
  the handoff changed; no production caller, route, provider integration,
  or V3 arming; `Cargo.lock` unchanged; no `rustfmt::skip`.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `8e50669`, clean worktree, branch `implement/impl-10857-vtoqyc8o`;
- both in-container verifies fully green; the repair's advisory review
  APPROVE with two low-risk DEFERs (regression-strength gaps) — judge
  whether either DEFER hides a blocker;
- operator host gates on exact `8e50669` all exit 0: `git diff --check`,
  formatter, locked all-target/all-feature workspace check and Clippy with
  `-D warnings`, full locked all-feature workspace test **4,060 passed / 0
  failed / 13 ignored across 90 harnesses**, locked release build,
  `cargo deny check`, and repository hygiene 40 tracked / 8 configs.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must name
  a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the two round-1 blockers as FIXED, PARTIAL, or
  OPEN against the shipped line.
- Judge the three operator adjudications.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- `crates/bridge-core/src/fs_custody.rs`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the Task C implementer statements)
- repository `AGENTS.md`
