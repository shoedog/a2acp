---
task-type: implement
---

# R2f1b slice 4F — durable fixed-grace timer (gated)

## Description

The sixth sub-slice of R2f1b slice 4. R2f1a **refuses** production `fixed_grace` before effects.
R2f1b lifts exactly that refusal under `AutomaticR2f1b` and arms a real, **one-shot, non-renewable**
grace timer that records a separately named policy trigger and **never rewrites the sibling's
recorded node deadline** (scope document §4.5, invariant 3 / D1).

**Built and tested; not yet reachable from production.** Readiness ships `Disarmed`, so
`AutomaticR2f1b` never occurs on a production path and the refusal continues to fire exactly as it
does today. `crates/bridge-workflow/src/executor.rs` stays byte-identical, including the bare
`let Some(first) = inflight.next().await` that is issue #22.

Base: `origin/main` = `63134836` (R2f1b slice 4E).

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` (§4, sub-slice 4F).
Scope document: `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` §4.5.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails on the base tree:

- `resolve_execution_policy_v1` contains, after building `controls`:
  `if let FanOutPolicyV1::FixedGrace { grace_ms } = controls.fan_out { if matches!(activation, PolicyActivationV1::Production) { return Err(ExecutionPolicyError::FixedGraceInactive); } … }`
- `FrozenWorkflowControlsV1.deadline_activation` is a `DeadlineActivationV2` produced by
  `deadline_activation_v2_for(readiness, activation)`.
- `PolicyTriggerV1` exists with fields `schema_version`, `id: ControlEventIdV1`,
  `node: PolicyNodeRefV1`, `policy: FanOutPolicyNameV1`, `grace_ms: Option<u64>`.
- `ExecutionPolicyError::InvalidFixedGrace` exists and is returned when `grace_ms == 0` or
  `grace_ms > controls.effective_work_cutoff_ms()`.

**Do NOT stop for immaterial measurement differences** — line numbers, diff counts, formatting-only
deltas. Cite by symbol, never by line.

### Verified anchors — operator-measured on this base

- The refusal fires for **every** `PolicyActivationV1::Production`, unconditionally on activation.
  Because 4B's readiness ships `Disarmed`, making the refusal conditional on `AutomaticR2f1b` leaves
  production behaviour **identical** — that is the whole meaning of "gated" here.
- The bounds check (`grace_ms == 0`, `grace_ms > effective_work_cutoff_ms()`) sits **after** the
  refusal, so today it is unreachable under `Production`. Lifting the refusal makes it reachable;
  it must stay enforced.
- `PolicyTriggerV1` already carries the "separately named trigger" vocabulary — a `ControlEventIdV1`
  and `FanOutPolicyNameV1`. Do not invent a parallel trigger type.
- Nothing arms any timer anywhere in this workspace today.

## What this sub-slice does

**1 — Lift the refusal, conditionally.**

Refuse `FixedGrace` when the frozen `deadline_activation` is `ManualOnlyR2f1a`, exactly as today.
Admit it under `AutomaticR2f1b`. The `InvalidFixedGrace` bounds check must remain enforced on the
admitted path — it becomes reachable for the first time here, so it must be tested for the first time
here too.

**2 — A one-shot, non-renewable grace timer as a durable state machine.**

Pure and clock-fed: it takes elapsed milliseconds, it does not read a clock, spawn, sleep, or select.

- It arms **once**. A second arm is refused or unrepresentable — prefer unrepresentable.
- It fires **once**. After firing it cannot be re-armed, and cannot fire again.
- Its state is durable: it round-trips through its serialized form. Because this state may be
  persisted, prove encoding stability against **literal bytes**, not a round-trip of a freshly built
  value.

**3 — A separately named trigger.**

Firing records a `PolicyTriggerV1` with its own `ControlEventIdV1` and
`FanOutPolicyNameV1::FixedGrace`. Distinct triggers must not collide.

**4 — Never rewrites the sibling's recorded node deadline (invariant 3 / D1).**

This is the invariant most likely to be violated by accident, so prove it directly: firing produces a
trigger and nothing else. A test must show that a node's recorded deadline is **unchanged** across
arm-and-fire — assert the deadline value, not merely that no error occurred.

## Invariants — must not change

- Production behaviour is **identical** to the base tree. Readiness ships `Disarmed`, so no production
  caller reaches the lifted path. A test must demonstrate the refusal still fires under
  `(Disarmed, Production)`.
- `crates/bridge-workflow/src/executor.rs` is **untouched**.
- No timer is armed from production; nothing spawns, sleeps, selects, or cancels.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched. If a change is
  genuinely unavoidable, **stop and report** rather than deciding it silently.

**The refusal gate (decomposition §5).** Re-assert, as 4B–4E did, that no production caller can
construct an automatic attempt while readiness is `Disarmed`.

**Carried from 4B and still binding:** "fully refused" is an *admission-layer* property. Do not add a
second production entry point to `resolve_execution_policy_with_readiness_v1`.

## Out of scope

- Wiring the timer into the executor or the arbitration readiness — 4H.
- Progress epochs and warnings — 4G. Issue #22 closure — 4I. Arming — 4J.
- Changing `PolicyTriggerV1`'s shape, or any persisted encoding other than the new timer state.

## Required tests

Each must fail on the pre-change tree — verify that, do not assume it:

1. `(Disarmed, Production)` still refuses `FixedGrace` with `FixedGraceInactive` — production
   behaviour unchanged.
2. Under `AutomaticR2f1b`, `FixedGrace` is admitted.
3. On that admitted path, `grace_ms == 0` still yields `InvalidFixedGrace`.
4. On that admitted path, `grace_ms > effective_work_cutoff_ms()` still yields `InvalidFixedGrace`.
5. The timer arms once; a second arm is refused or does not compile.
6. The timer fires once; after firing it can neither re-arm nor fire again.
7. Firing records a `PolicyTriggerV1` with `FanOutPolicyNameV1::FixedGrace` and its own id; two
   triggers do not collide.
8. **The recorded node deadline is unchanged across arm-and-fire** — assert the value.
9. Durable encoding stability against literal bytes.
10. The refusal gate, as in 4B–4E.

## Size

**Cap: 300 counted lines** (added nonblank physical Rust lines after `cargo fmt`, docs excluded).
Projection: 200. The cap is a **stop boundary**, not a target. If the change cannot be made within it,
**stop and report** rather than growing it.

## Frozen single-mutation control

Produce a patch reverting exactly one **production** change, record its SHA-256, and verify:

- it applies cleanly to the candidate tree;
- it reddens at least one named test — report the **actual** red population from a **full-suite** run,
  computed as the set difference against the candidate's own pre-existing failures;
- the mutated tree still passes `cargo clippy --all-targets --all-features --locked -- -D warnings`.

Prefer **making the timer renewable** — allowing a second arm after firing. One-shot-ness is this
sub-slice's whole safety property, and a control that breaks it is the strongest available.

If the container cannot fetch crates, use the warm cache offline —
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true` with localhost excluded from the injected proxy, and an
explicit `RUSTDOC`. Report doc-test launch failures separately; they are environmental.

## Handoff

Write `docs/superpowers/reviews/2026-08-24-r2f1b-slice4f-handoff.md` covering: what changed, the
control patch path and SHA-256, the actual red population, the deliberate exclusions, and the counted
line total against the cap.

**Report gate results truthfully.** If the configured test command is not green, say so and name the
failing test. If a fixture or expectation was hand-written rather than tool-generated, say so.
Exclude diagnostic runs that failed for their own reasons from the gate evidence, and name them.

**Note on the host suite:** nine `tests/smoke_cli.rs` / `tests/fallback_plan_cli.rs` failures are
environmental and intermittent on this lane — present for 4A/4B and 4E, absent for 4C/4D. If you see
them, report them; do not chase them into production changes.

**Do not record your own head or tree sha.**

## Acceptance Criteria

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`,
  `cargo build --locked`, and the configured test command are all green.
- Every test in "Required tests" exists and fails on the pre-change tree.
- Production behaviour is identical to base: the refusal still fires under `(Disarmed, Production)`.
- The timer cannot be armed twice or fired twice.
- `executor.rs` is byte-identical to the base.
- Counted added nonblank Rust lines ≤ 300.

## Files

- `crates/bridge-core/src/execution_policy.rs` — the conditional refusal.
- `crates/bridge-core/src/` — the timer state machine (a new module is fine).
- Test files under `crates/bridge-core/tests/`.

## Spec Refs

- `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` — plan of record.
- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` — §4.5 fixed grace, invariant 3 / D1.

## Commit Message

Build the gated one-shot fixed-grace timer (R2f1b slice 4F)
