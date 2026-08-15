---
task-type: implement
---
# R2f1b 3c2 Task F2: remove the retained shared-flight adapter

## Description

Begin Task F2 — the named split of Task F — on the exact accepted Task F
head `15912e3ab4f3a2c39bbe599d91010fe3f945b9f5`. Task F migrated the API
send path onto the owned `RemoteRequest*` mechanism and left the old
shared-flight request adapter private and unreferenced behind seven
item-scoped `#[allow(dead_code)]` annotations, each naming F2. This task
performs that removal. Pure deletion plus the annotation removal: zero
behavior change anywhere. Production remains `LegacyV2` with the V3
route unarmed.

Own `crates/bridge-core/src/process.rs`,
`crates/bridge-core/src/retained_resource_flight.rs`, any request-only
test fixtures that still construct the deleted items, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not modify bridge-api, `remote_request_flight.rs`, the Task A-E
surfaces, workflow/worktree crates, or `bin/`.

Remove exactly the retained adapter population (the operator
host-enumerated all seven dead-code items; the closure confirmed no
production reference outside the defining adapter):

- `DurableRemoteRequestFlightV3` (struct, its impl including
  `request_id`, `flight_id`, `settlement_handle`, `begin_dispatch`,
  `settle`, its `Drop` impl, and its `Debug` impl);
- `RemoteRequestSettlementV1` (struct, `join_blocking`, and its impl);
- `RemoteRequestFlightErrorV1` (the bridge-core enum, its Display and
  Error impls) — bridge-api owns its own `ApiRequestFlightErrorV1` and
  must be untouched;
- `bind_remote_request` and the old adapter's route struct/fields it
  alone used (`with_result_publisher` on that dead route included, if
  nothing live uses it);
- `attach_remote_request_owner` on the retained flight;
- every `#[allow(dead_code)]` annotation Task F added for the above.

If deleting an item would force ANY change to live production behavior
or to a surviving public signature, stop and report instead of adapting
around it — that would mean the census was wrong.

## Acceptance Criteria

- This is a deletion task: the required evidence is a reference census
  (zero references to every deleted symbol workspace-wide, tests
  included) plus green gates WITHOUT the allowances — record the exact
  search commands. No new tests are required; no existing test may
  change behavior. A test that constructed a deleted item may only be
  deleted if it tested ONLY the deleted adapter — name each such test
  in the handoff.
- `cargo clippy --workspace --all-targets --all-features --locked --
  -D warnings` exits 0 with the allowances gone.
- Run `cargo test -p bridge-core --lib -- remote_request_flight process
  retained_resource_flight` and `cargo test -p bridge-api` (all
  harnesses), plus `git diff --check` and `cargo fmt --all -- --check`;
  no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `15912e3a`, the census
  evidence, honest churn accounting (additions plus deletions,
  post-format), and the statement that Task G and production V3 remain
  unarmed.
- Stop and report before exceeding **50 added production lines or 600
  total changed lines** (churn convention; deletions dominate by
  design) relative to `15912e3a`.

## Files

- `crates/bridge-core/src/process.rs`
- `crates/bridge-core/src/retained_resource_flight.rs`
- request-only test fixtures of the deleted items (if any remain)
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; the binding F2 clause: "land migration with the old
  adapter private/unreferenced, then remove it in F2 before any review
  of the aggregate")
- repository `AGENTS.md`

## Commit Message

refactor(r2f1b): delete the retired shared-flight request adapter

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator
will first classify it: only a closed, enumerable rejection may receive
one targeted repair on this same artifact followed by one closure
review. An open-class or repeating family parks F2. Never restart from
a fresh artifact and never silently extend the cap.
