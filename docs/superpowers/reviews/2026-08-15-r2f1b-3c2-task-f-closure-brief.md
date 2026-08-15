---
task-type: code-review
---
# R2f1b 3c2 Task F closure review

## Description

Perform the one counted closure review of the complete Task F line:
exact diff `a1f1f8de..15912e3a` in this checkout, where `a1f1f8de` is
the accepted Task E head and `15912e3a` is the current head. This is
the closure declared by the Task F round contract; it is capped at one
pass with no repair loop inside it.

The line has three commits:

1. `f17e2958` — the base implement: the API send path migrated onto the
   Task B-D `RemoteRequest*` mechanism (atomic admission, the owned
   driver, the first-poll arming fence), the exact delivery-ID
   publication acknowledgement wired into the Task E cleanup cell (the
   positive `Complete` projection Task E left pending), and the salvage
   plan's F2 SPLIT taken: the old shared-flight adapter left private
   and unreferenced, with its removal named F2 (due before the
   aggregate round). Advisory review REJECTed on two WRONGs: (1)
   pre-send exits keyed their disposition on `dispatched` (set at
   dispatch AUTHORIZATION) rather than the arming marker, settling
   unarmed rows `Partial,false`/`Unknown,false` instead of the recovery
   table's `Failed,false`; (2) the retained adapter failed the
   workspace `-D warnings` gate as dead code (operator host-enumerated:
   exactly seven symbols).
2. `7d3202cf` — the contracted targeted repair (34 production/149
   total): `acceptance_keyed_disposition` normalizes every unaccepted
   exit to `Failed`; the request bit is set by the first-poll
   `RequestAcceptanceMarker`; the seven F2 allowances are item-scoped,
   each naming F2; the handoff churn understatement (367 → 371) was
   corrected. Its advisory review confirmed the first-round repair and
   the scoped allowances but REJECTed on exactly one fresh WRONG:
   `attach_lifecycle` copied the turn-wide `acceptance_barrier_crossed`
   into the REQUEST-LOCAL bit, so successor tool-call rounds were
   pre-marked accepted — a round-two cancel/drop before its own first
   poll still persisted `Partial,false`/`Unknown,false`.
3. `15912e3a` — the disclosed operator completion (production +5/−6,
   tests +151, handoff +52): `attach_lifecycle` now propagates the
   sticky turn acceptance ONLY to the cleanup cell's diagnostic custody;
   the request-local bit is set solely by the first-poll marker; the
   now-unused `RequestScope::mark_accepted` is deleted. Red-first: the
   successor cancel and drop regressions failed behaviorally on
   `7d3202cf`; a public two-round accepted-edge test (`Complete,true`
   then in-flight cancel `Partial,true`) is green on both heads and
   guards against overcorrection.

Adjudicate:

- the base round's two WRONGs and the repair round's successor
  inheritance WRONG as FIXED, PARTIAL, or OPEN against `15912e3a`;
- the operator completion at source — it is operator-authored and you
  are licensed to reject it. Disclosed operator concerns for contest:
  (a) the sticky-to-cell-only propagation preserves turn-level
  `prompt_may_have_been_accepted` diagnostics (the cell ORs its
  acceptance) while the durable row records the request-local truth —
  falsify either half; (b) deleting `RequestScope::mark_accepted`
  leaves the first-poll marker as the sole setter — find any path that
  legitimately needed the old propagation (a settle path that reaches
  `acceptance_keyed_disposition` with a rightly-accepted request whose
  marker never ran); (c) the accepted-edge public test pins
  `Partial,true` for in-flight round-two cancellation — judge whether
  any accepted flow was narrowed;
- the F2 split state: the seven allowances are item/impl-scoped and
  each names F2; the old adapter has zero external references; F2
  removal remains due before the aggregate round — confirm nothing
  else leaks;
- the salvage plan's Task F criteria: zero-round/never-polled sends do
  not mint or admit; every terminal path records the recovery-table
  result (unaccepted exits `Failed,false`; armed `Unknown,true`);
  the first-poll fence controls acceptance; cancellation between
  rounds prevents the successor send; a fully successful V3 request
  with the exact-echo acknowledgement projects checked cleanup
  `Complete` and stays `Unknown` without it; process/container focused
  suites unchanged outside the owned request-specific sections;
- the advisory SMELL (API-level first-poll ordering at the real
  reqwest future; rejected/mismatched-echo acknowledgement projecting
  `Unknown`) — judge whether it hides a blocker; otherwise it goes to
  the aggregate ledger;
- the `remote_request_flight.rs` +6 — the operator verified it is a
  semantics-free `Debug` impl for `RemoteRequestDriverV1`; falsify if
  it changes behavior;
- scope: across the whole line only bridge-api (backend, config), the
  narrow bridge-core visibility/allow/Debug changes, and the handoff
  changed; `Cargo.lock` unchanged; no `rustfmt::skip`; production
  construction still assigns the V3 route `None` and exposes
  `LegacyV2`; the Task A-E surfaces are semantically untouched.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `15912e3a`, clean worktree, branch
  `implement/impl-11621-7jfy4top`;
- the repair's in-container verify was fully green; the base run's
  in-container whole-bin failure carried the ledgered flock-EBADF
  hermetic signature (instance 8) with a host control on exact
  `f17e2958` of 1,086 passed / 0 failed;
- operator host gates on exact `15912e3a` all exit 0: `git diff
  --check`, formatter, locked all-target/all-feature workspace check
  and Clippy with `-D warnings`, full locked all-feature workspace test
  **4,090 passed / 0 failed / 13 ignored across 90 harnesses**, locked
  release build, `cargo deny check`, and repository hygiene.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must
  name a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the three prior blockers, the operator
  completion, and the F2 split state, and confirm no regression in the
  previously sustained Task A-E families reachable from this line.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-api/src/backend.rs`
- `crates/bridge-api/src/config.rs`
- `crates/bridge-core/src/process.rs`
- `crates/bridge-core/src/retained_resource_flight.rs`
- `crates/bridge-core/src/remote_request_flight.rs`
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; the salvage plan file is NOT in this checkout — the
  binding Task F contract is restated in full in this brief's
  Description and criteria, including the F2 split clause: "if HTTP
  migration and old-adapter removal cannot fit, land migration with the
  old adapter private/unreferenced, then remove it in F2 before any
  review of the aggregate")
- repository `AGENTS.md`
