---
task-type: code-review
---
# R2f1b 3c2 Task D final closure review

## Description

Perform the one owner-authorized final closure review of the complete Task
D line: exact diff `832221c9..2697c438` in this checkout, where `832221c9`
is the accepted Task C head and `2697c438` is the current head. This is
the closure of the owner-authorized additional repair round; it is capped
at one pass with no repair loop inside it.

The prior closure (of head `08aa5531`) adjudicated the round-1 and
repair-round blockers FIXED/PARTIAL, accepted the conservative residual
and the per-wrapper first-poll fence, and rejected on exactly two fresh
concurrency blockers:

1. duplicate wrappers: `arm_provider_send` had no request-wide one-shot,
   so a second wrapper's failed arm could reach the zero-poll privilege
   and durably publish `Failed, accepted=false` over a genuinely sent
   request;
2. false-success settlement: a settler losing the boolean publication
   claim returned `Ok(outcome)` while the winner's publication could still
   refuse, and a stale completion precheck could let a late claimant
   republish after retirement.

The new final commit `2697c438` (+286/−20 module, +50 handoff) implements
the prescribed repairs, red-first (both regressions failed on `08aa5531`;
its advisory review returned APPROVE — "both concurrency blockers are
correctly repaired" — with two low-exposure regression-robustness DEFERs):

- an irreversible request-wide send permit: only the first wrapper to
  claim may arm or ever use the `failed_arm` privilege; later wrappers
  destroy their inner futures unpolled and return a typed refusal with
  zero row effect; the permit does not release on drop;
- a joinable publication flight replacing the boolean claim: every
  concurrent settler receives the same completed publication result
  (success or the winner's refusal), and completion is rechecked after
  claim acquisition so a post-retirement claimant neither republishes nor
  errors.

Adjudicate:

- the two prior-closure blockers as FIXED, PARTIAL, or OPEN against
  `2697c438`, including the barrier-refusing-publisher and success-race
  schedules;
- that the permit does not break the legitimate single-wrapper flows
  (arming, effect-then-debt settlement, drop semantics) or the recovery
  table;
- that the joined publication result preserves the outbox discipline
  (pending blocks admission; exact acknowledgement echo; no republish
  after retirement);
- the two advisory DEFERs — judge whether either hides a blocker;
- scope: only the owned module and handoff changed across the final
  commit; no production caller, provider integration, or V3 arming;
  `Cargo.lock` unchanged; no `rustfmt::skip`.

All prior-line adjudications (first-poll fence, conservative residual,
recovery truthfulness, CAS-winner discipline, observation bounds, refusal
debt, peer isolation) were sustained by the prior closure and are not
reopened unless you find a new constructible WRONG.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `2697c438`, clean worktree, branch `implement/impl-55963-xnixoil5`;
- in-container verify fully green on this run;
- operator host gates on exact `2697c438` all exit 0: `git diff --check`,
  formatter, locked all-target/all-feature workspace check and Clippy with
  `-D warnings`, full locked all-feature workspace test **4,073 passed / 0
  failed / 13 ignored across 90 harnesses**, locked release build,
  `cargo deny check`, and repository hygiene 40 tracked / 8 configs.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must name
  a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the two prior blockers and confirm no regression
  in the previously sustained families.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`
