---
task-type: code-review
---
# R2f1b 3c2 Task G closure review

## Description

Perform the one counted closure review of the complete Task G line:
exact diff `f17e2bd3..f04ec55e` in this checkout, where `f17e2bd3` is
the accepted Task F2 head and `f04ec55e` is the current head. This is
the closure declared by the Task G round contract; it is capped at one
pass with no repair loop inside it.

The line has three commits:

1. `4c8e408b` — the base implement (181 production/541 total): exact
   `BackendCleanupDispositionV1` returned from `cleanup_cold_session`
   with a caller/wrapper enumeration; node retry gated on exact
   `Complete` (`Ok(Unknown)` cannot redispatch, red-first at the three
   node retry sites); post-acceptance persistence failure
   fatal/nonretryable; an exhaustive bridge-worktree guard pinning both
   disposition sets, both `CleanupReportV1` fields, and the full fold
   cross-product (only exact `Complete + Complete` folds `Complete`);
   the production-route assertion (production V3 route `None`) as a bin
   test. Advisory review REJECTed on: (W1) the preflight run cache
   evicted failures whose cleanup was NOT proven complete — a configure
   or provably-unaccepted prompt failure with cleanup `Unknown`/
   `Retained`/`Preserved`/error exited the "preflight exhausted"
   terminal with `retain_in_run_cache: false`, so a later node could
   reconfigure and prompt the same logical session; (W2) the caller
   enumeration found a second collapsing consumer — smoke's generic
   `cleanup_step` maps every `Ok(T)` to artifact `"completed"` — which
   is outside G's owned paths, and the binding one-consumer-per-task
   clause required naming a split, which the base handoff failed to do;
   (W3, DEFER-classed WRONG) the failure-reason match lacked a
   `Some(Ok(Complete))` arm, so complete cleanup masked the real
   empty/unexpected-response reason with `cleanup incomplete:
   Complete`.
2. `be7baa29` — the contracted targeted repair (executor +131/−37,
   handoff +79/−1; 248 total): retention keyed on proven-clean —
   `retain_exhausted_failure = !cleanup_proven_complete` at BOTH break
   sites (configure error and prompt-failure paths) feeding the
   exhausted terminal, with the documented invariant that a failure is
   evicted only when it is both pre-acceptance AND proven clean; the
   `Some(Ok(Complete)) | None` reason arm preserving the
   response-derived failure; and the handoff's smoke-consumer census
   naming **G2** as the separate follow-up slice with an explicit
   wire-compatibility boundary (no `smoke.rs` code change, per
   ownership). The fault-backend scaffolding gained a `Configure`
   fault, injectable typed cleanup results, and configure/prompt
   counters for the per-disposition red regressions. Its advisory
   review returned APPROVE with two DEFERs: the undiscriminated
   aggregate-test red (since classified — see supplied evidence) and a
   missing configure-clean eviction regression (the negative space:
   pre-acceptance + proven-clean still evicts).
3. `f04ec55e` — a disclosed operator gate-repair (test-only, +7/−1, in
   `crates/bridge-api/src/backend.rs` — OUTSIDE Task G's owned paths,
   which is itself disclosed for your judgment): the Task E-era
   public-path crossing test set `request_timeout` to 200 ms, one knob
   that bounds BOTH the HTTP round and the cleanup-cell deadline. Under
   full-suite parallel load the HTTP round exceeded 200 ms, ending the
   drain without `Done("stop")`. The operator's full host gate failed
   there twice on `be7baa29`, the test ran 10/10 green in isolation on
   the same head, and a same-environment BASE control (the full
   workspace suite at accepted `f17e2bd3`) failed at the SAME test and
   assertion — proving the defect pre-existing and not G-attributable.
   The fix raises the bound to 2 s with a comment stating the
   invariant: the test's determinism comes from its barriers (the
   publisher stays stalled until released, so the deadline always
   expires mid-settlement), not from the clock — the crossing property
   the E closure accepted is unchanged, only the wait lengthens. This
   is the same hardening class the F2 closure prescribed for the
   signal-semantics test.

Adjudicate:

- the base round's W1/W2/W3 as FIXED, PARTIAL, or OPEN against
  `be7baa29`, including whether the retention now covers every
  pre-acceptance path (trace both break sites and any other exit from
  the candidate loop) and whether any legitimate eviction was lost
  (the accepted-prompt never-replay retention must be unchanged; the
  pre-acceptance proven-clean eviction must still happen);
- the G2 naming as satisfying the binding split clause — the smoke
  `cleanup_step` collapse itself is NOT to be fixed in this line;
- the advisory's second DEFER (missing configure-clean eviction
  regression) — judge whether it hides a blocker; otherwise it goes to
  the aggregate ledger;
- the operator gate-repair: judge whether raising the test's bound from
  200 ms to 2 s weakens anything the Task E crossing test proves (the
  operator's claim: the barriers carry the determinism, so nothing is
  weakened), and whether a cross-crate test-only change was the right
  disposition given the same-environment base-red control;
- the Task G binding criteria across the full line: `Ok(Unknown)`
  cannot redispatch at any node or preflight consumer;
  post-acceptance persistence failure is fatal/nonretryable; worktree
  inner/checkout outcomes remain separate with the two-field
  `CleanupReportV1` contract byte-compatible; the production API route
  assertion holds;
- scope: across the line only `crates/bridge-workflow/src/executor.rs`,
  `crates/bridge-worktree/src/backend.rs` (guard tests),
  `bin/a2a-bridge/tests/r2f0b_production_wiring.rs` (assertion test),
  the one-line test hardening in `crates/bridge-api/src/backend.rs`
  (test module only), and the implementer handoff changed; `Cargo.lock`
  unchanged; no `rustfmt::skip`; no production code change outside
  `bridge-workflow`; production V3 remains unarmed.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `f04ec55e`, clean worktree, branch
  `implement/impl-28424-ayf02m4i`;
- the repair's in-container verify failed ONLY at the whole-bin
  `a2a-bridge` test target with the ledgered flock-EBADF hermetic
  signature (`authority-state.lock`/`owner-admission.lock`, os error
  9) — instance 9 of the class; the operator host control on exact
  `be7baa29` ran the whole-bin target **1,086 passed / 0 failed** (the
  class is 9/9 host-green); fmt, clippy `-D warnings`, and build were
  green in-container;
- the operator's first two full host gate runs on `be7baa29` failed at
  the Task E crossing test under load; the 10/10 isolated control and
  the same-environment base-red control at `f17e2bd3` are logged
  (`g-flake-probe.log`, `g-base-load-control.log`);
- operator host gates on exact `f04ec55e` all exit 0: `git diff
  --check`, formatter, locked all-target/all-feature workspace check
  and Clippy with `-D warnings`, full locked all-feature workspace test
  **4,093 passed / 0 failed / 13 ignored across 90 harnesses** (under
  the same load profile that failed twice pre-hardening), locked
  release build, `cargo deny check`, and repository hygiene.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must
  name a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the three prior findings and the retention
  completeness, and confirm no regression in the previously sustained
  Task A-F2 families reachable from these files.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-workflow/src/executor.rs`
- `crates/bridge-worktree/src/backend.rs`
- `bin/a2a-bridge/tests/r2f0b_production_wiring.rs`
- `crates/bridge-api/src/backend.rs` (one-line test hardening only)
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; the binding Task G contract is restated in this
  brief — the salvage plan file is not in this checkout's lineage)
- repository `AGENTS.md`
