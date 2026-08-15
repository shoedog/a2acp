---
task-type: code-review
---
# R2f1b 3c2 Task F2 closure review

## Description

Perform the one counted closure review of the complete Task F2 line:
exact diff `15912e3a..f17e2bd3` in this checkout, where `15912e3a` is
the accepted Task F head and `f17e2bd3` is the current head. This is the
closure declared by the F2 round contract; it is capped at one pass with
no repair loop inside it.

The line has two commits:

1. `b3e354ab` — the deletion: the retired shared-flight request adapter
   removed exactly (production +3/−395: `DurableRemoteRequestFlightV3`
   with its impls, `RemoteRequestSettlementV1`, the bridge-core
   `RemoteRequestFlightErrorV1`, `bind_remote_request`,
   `attach_remote_request_owner`, and all seven F2-scoped
   `#[allow(dead_code)]` annotations). Two tests deleted because they
   exercised only the deleted seam (named in the handoff); one mixed
   test kept with only its retired fixture removed. The advisory review
   verified the deletion correctly scoped — census clean, live
   `RemoteRequestDriverV1` path byte-identical, surviving public
   signatures untouched — and REJECTed on exactly ONE delivery finding:
   the handoff recorded the mandatory focused core selector red
   (128/129, twice, at
   `term_ignoring_child_with_descendant_is_group_killed_host_signal_semantics`)
   without a green exact-command run.
2. `f17e2bd3` — the disclosed operator completion (docs only, +30
   handoff lines, zero code): the exact post-commit host run of the
   required selector on `b3e354ab` is green — **129 passed / 0
   failed**, the disputed test explicitly ok (log
   `f2-focused-host.log`) — and the two container reds are classified
   with a same-environment control: the same container session's later
   full verify test stage ran GREEN (the identical test passed in the
   identical environment after the failures), the test is byte-identical
   to the frozen base, the diff is deletion-only touching no
   signal/process-group code, and the same sole in-container failure
   was already recorded once on the Task F line. Classification:
   container-environment signal-semantics flake (process-group kill
   visibility), host-green on every exact-command run.

Adjudicate:

- the advisory blocker (focused gate not green) as FIXED, PARTIAL, or
  OPEN against `f17e2bd3` given the dated exact-command run, the
  same-environment control, and the classification — this was the
  reviewer's own stated collapse condition;
- the deletion itself: reconfirm or falsify the census (zero
  workspace-wide references to every deleted symbol), the two deleted
  tests' only-deleted-seam justification, the mixed test's retained
  assertions, and that no live production behavior or surviving public
  signature changed;
- the operator completion contains zero code change;
- scope: across the line only `crates/bridge-core/src/process.rs`,
  `crates/bridge-core/src/retained_resource_flight.rs`, and the
  implementer handoff changed; `Cargo.lock` unchanged; no
  `rustfmt::skip`; production construction still assigns the V3 route
  `None` and exposes `LegacyV2`; the Task A-F surfaces are semantically
  untouched.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `f17e2bd3`, clean worktree, branch
  `implement/impl-68499-l8h4n1rv`;
- the deletion commit's in-container verify was fully green (fmt,
  clippy with the allowances gone, build, workspace test);
- operator host run of the exact focused selector on `b3e354ab`: 129
  passed / 0 failed;
- operator host gates on exact `f17e2bd3` all exit 0: `git diff
  --check`, formatter, locked all-target/all-feature workspace check
  and Clippy with `-D warnings`, full locked all-feature workspace test
  **4,088 passed / 0 failed / 13 ignored across 90 harnesses** (down
  exactly the two deleted adapter-only tests from Task F's 4,090),
  locked release build, `cargo deny check`, and repository hygiene.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must
  name a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the advisory blocker and the deletion census,
  and confirm no regression in the previously sustained Task A-F
  families reachable from these files.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/process.rs`
- `crates/bridge-core/src/retained_resource_flight.rs`
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; the binding F2 clause: remove the retained adapter
  before any review of the aggregate)
- repository `AGENTS.md`
