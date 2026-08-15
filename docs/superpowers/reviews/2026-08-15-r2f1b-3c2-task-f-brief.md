---
task-type: implement
---
# R2f1b 3c2 Task F: migrate API request execution and remove the shared-flight adapter

## Description

Begin Task F on the exact accepted Task E head
`a1f1f8de8052385ecc837c6950fe856e331e65de`. Migrate the actual API send
path onto the Task B-D `RemoteRequest*` mechanism and remove the old
shared-flight request adapter. Production remains `LegacyV2` with the V3
route unarmed; no HTTP behavior changes for Legacy execution.

Own: `crates/bridge-api/src/` (config, backend, lib, and a new request
module if you split one out); the REQUEST-ONLY portions of the shared
bridge-core process/resource/retained-flight core (exactly what the
adapter removal below reverts — nothing else in those files); the
request-specific sections of the bridge-core
process/resource/retained-flight/reaper tests; Cargo manifests only if
required; and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not modify the Task A-C custody surfaces
(`fs_custody`, `namespace_transaction`, journal grammar), the Task D
`remote_request_flight` module's semantics, workflow/worktree crates, or
`bin/` production wiring.

Implement, per the binding salvage design:

- **route migration:** change the injected V3 route to carry the Task
  B-D attempt (`Arc<RemoteRequestAttemptV3>`-shaped authority from
  `bridge-core::remote_request_flight`); mint/admit through the Task B
  atomic admission; wrap the actual send future in the Task D owned
  driver so `ProviderSendArmed` becomes durable immediately before the
  FIRST poll of the installed send future (the first-poll arming
  fence); migrate every V3 test onto the new mechanism;
- **honest acknowledgement completed:** results settled through the new
  mechanism record `acknowledged=true` in the cleanup cell ONLY from
  the exact delivery-identity publication acknowledgement (Task C/D
  outbox echo); Task E's `acknowledged=false` stays for any path
  without that echo — this is what turns the cell's V3 `Complete`
  projection truthful;
- **adapter removal:** remove the old remote request driver
  (`DurableRemoteRequestFlightV3` and its bind/settle surface) and
  revert the request-only reservation/recovery/operation-lock/
  publication additions to the shared process/container flight core;
  the shared core keeps only what processes/containers need. Preserve
  the 3c2 identity, ABA, cancellation, lifecycle, and post-acceptance
  error repairs (including Task E's cleanup cell semantics — absorbing
  `TimedOut`, drop custody transfer, the exact projection table);
- if the full HTTP migration and the old-adapter removal cannot both
  fit the caps, land the migration with the old adapter private and
  unreferenced, name F2 for the removal, and stop — do not squeeze.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands
  and admissibility. A compile failure counts only when it is
  specifically the missing Task F API; zero selected tests does not.
- Zero-round / never-polled send does not mint an identity or admit a
  journal row.
- Every send/error/SSE/unary terminal path records the expected durable
  result (the Task D recovery table: pre-send `Failed,false`;
  `ProviderSendArmed` `Unknown,true`).
- The first-poll fence controls acceptance: arming is durable before
  the first inner poll, never on wrapper construction.
- Cancellation between rounds prevents the successor send (no mint, no
  admission, no durable row for the cancelled round).
- A fully successful V3 request with the real publication
  acknowledgement projects checked cleanup `Complete`; without the
  acknowledgement it stays `Unknown` (extends Task E's no-op-publisher
  regression to the positive exact-echo case).
- All old request adapter symbols have zero references (or, under the
  F2 split, are private and unreferenced with F2 named in the
  handoff); process/container focused tests remain unchanged except
  the request-specific sections named in ownership.
- Run `cargo test -p bridge-api` (all harnesses),
  `cargo test -p bridge-core --lib -- remote_request_flight process
  retained_resource_flight reaper`, plus `git diff --check` and
  `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `a1f1f8de`, red evidence,
  honest churn accounting (additions plus deletions, post-format), and
  the statement that Task G and production V3 remain unarmed.
- Stop and report a split before exceeding **500 changed production
  lines or 900 total changed lines** (churn convention) relative to
  `a1f1f8de`. The F2 escape hatch above is the named split.

## Files

- `crates/bridge-api/src/` (config, backend, lib, new request module)
- `crates/bridge-core/src/process.rs` and
  `crates/bridge-core/src/retained_resource_flight.rs` (request-only
  removal/revert sections exclusively)
- request-specific sections of bridge-core process/resource/
  retained-flight/reaper tests
- Cargo manifests only if required
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/plans/2026-08-12-r2f1b-3c2-salvage-redesign.md`
  (section "F. Migrate API request execution and remove the
  shared-flight adapter" — binding)
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

feat(r2f1b): migrate API requests onto the owned flight and drop the adapter

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator
will first classify it: only a closed, enumerable rejection may receive
one targeted repair on this same artifact followed by one closure
review. An open-class or repeating family parks Task F. Never restart
from a fresh artifact and never silently extend the cap.
