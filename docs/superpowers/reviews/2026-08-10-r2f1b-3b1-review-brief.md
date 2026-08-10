---
task-type: code-review
---
# R2f1b 3b1 — process authority I: dual-lens review (sol lens)

## Description

Review the FULL 3b1 diff on this checkout: `git diff 0a3c2434..HEAD` (two
commits: base `bec2e01b` + continuation `45a72f01`; ~+3,600/−320 across
bridge-core/bridge-acp, one bridge-container initializer, one reaper test).
The slice makes process destruction capability-gated: `Supervised`
construction internal to `OwnedProcessTreeV1`; all three signal paths
(`Drop`, `terminate`, `terminate_blocking`) join-or-refuse through the 3a
`RetainedResourceFlight` runner; SIGSTOP-verified-stop containment closure
with child-first SIGKILL; both-platform start-identity probes; AcpBackend
adapter (cancel/escalate/retire/registry/Drop join; ordinary release
detaches); injectable process-authority port; spawn-caller migration; V2
paths byte-identical with controls; a diagnostic rider at the resolver's
`AnchoredProcessGroup::Drop`.

Review with the senior-lead posture: would you ship this? Practicality and
correctness balanced; a WRONG requires a concrete failure scenario (input or
state → incorrect result); a finding without one is a SMELL, never a
blocker; prefer DEFER-with-ledger over manufactured blockers. Acceptance
literalism against the spec's letter where the mechanism is sound is not a
blocker.

Load-bearing invariants to verify at mechanism level (not by checklist):

1. Containment closure: after the SIGSTOP volley, stability requires BOTH
   census-set equality AND every member verified stopped via the platform
   status probe (darwin `pbi_status == SSTOP`, Linux stat `T`) — a stopped
   process cannot fork, so the SIGKILL sweep is closed. Check the closure
   loop's termination, the `ContainmentUnstable` refusal, and whether any
   path can SIGKILL from an UNVERIFIED census.
2. The three signal paths all reach join-or-refuse; `terminate_blocking`
   joins from a bare `std::thread` via `join_blocking`/typed refusal (no
   async dependence); no eternal-hang window.
3. V2 byte-identity: the legacy arms must not enter V3 containment
   (`terminate_blocking_legacy_v2`, V2 Drop single-stage group SIGKILL);
   verify the controls actually pin this.
4. Kill-outcome observability: `settle_dispatch` maps `rc == -1 && errno !=
   ESRCH` to disposition `Failed`; the four ACP teardown consumers route
   results through the recorder fns — verify `Ok(Failed)` cannot be recorded
   as clean teardown anywhere.
5. Flight-before-spawn crash windows record protectively; the pid gate via
   the injectable port refuses same-pid/different-identity for EVERY flight
   action.
6. One registry per attempt (the D4 route binding: config field
   `process_flight_route_v3`, `spawn_with_durable_process_flight_v3`); no
   signal path derives targets from `ResourceFlightJournal::records()`.
7. The wire surface: any new/changed journal record shapes vs the 3a
   goldens; `deny_unknown_fields` discipline on new serialized types.

## Acceptance Criteria

- A verdict line `VERDICT: APPROVE|REJECT`.
- Findings tagged WRONG (with the concrete failure scenario) or SMELL,
  ordered most severe first, each with file/symbol anchors.
- Explicit mechanism verdicts on invariants 1–7 above (pass/fail/deferred,
  one line each).
- Do NOT re-report the known ledger (already adjudicated by the operator):
  (a) darwin host red `journal_then_spawn_fails_and_spawn_then_bind_fails_record_protectively`
  — the ImmutableStart bind-failure row lacks `pid: Some(_)` on darwin
  (repair-round item); (b) the D3 residual (trace fingerprint vs owner id;
  no flight-journal record for attach failure) — ledgered as SMELL; (c) the
  reaper timeout-test robustness edit — accepted rider; (d) the
  bridge-container one-line initializer — accepted ripple; (e) the
  fs_custody tripwire weakening — already byte-reverted. Findings that
  MATERIALLY EXTEND one of these (a new failure scenario, not a restatement)
  are welcome.

## Files

`crates/bridge-core/src/process.rs` (the bulk),
`crates/bridge-acp/src/acp_backend.rs`,
`crates/bridge-core/src/retained_resource_flight.rs`,
`crates/bridge-core/src/resource_flight.rs`,
`bin/a2a-bridge/src/compatibility_process_group.rs` (diagnostic rider),
`bin/a2a-bridge/tests/e2e_{delegate,fanout,kiro}*.rs` (spawn migration).

## Spec Refs

- `docs/superpowers/plans/2026-08-09-r2f1b-slice3-brief.md` §3 "3b1", §6.
- `docs/superpowers/reviews/2026-08-10-r2f1b-3a-dual-review.md` — "Ledger:
  3b1 (binding)".
- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` §5.4, §5.7
  rows 8–9.
