---
task-type: implement
---

# R2f1b slice 4G — progress epochs and no-progress warnings

## Description

The seventh sub-slice of R2f1b slice 4. It settles the **warning cadence** and, with it, the
invariant that gives the whole subsystem its safety: **silence never cancels.**

Cadence (scope document §4.3): `ordinal = floor((now - last_meaningful_progress) / 30m)`; each
positive ordinal emits **once per progress epoch**; activity without meaningful progress updates only
the activity clock; progress resets the epoch.

**This sub-slice arms no timer, adds no cancellation path, and changes no scheduling behaviour.**
`crates/bridge-workflow/src/executor.rs` stays byte-identical, including the bare
`let Some(first) = inflight.next().await` that is issue #22. Readiness ships `Disarmed`.

Base: `origin/main` = `23e331c6` (R2f1b slice 4F).

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` (§4, sub-slice 4G).
Scope document: `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` §4.3.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails on the base tree:

- `bridge_core::attempt_activity::ActivityReason` exists as an enum with the thirteen variants
  `PhaseTransition`, `MessageDelta`, `ThoughtDelta`, `UsageHighWater`, `ToolTransition`,
  `OwnedChildTransition`, `OwnedChildOutput`, `RepositoryOrdinal`, `GateStarted`, `GateExited`,
  `CompletedSetGrowth`, `ProducerTerminal`, `Heartbeat`.
- `crates/bridge-workflow/src/scheduler_arbitration.rs` exports
  `SchedulerArbitrationReadinessV1` carrying `no_progress_snapshot_due: bool`.

**Do NOT stop for immaterial measurement differences** — line numbers, diff counts, formatting-only
deltas. Cite by symbol, never by line.

### Verified anchors — operator-measured on this base

- `ActivityReason` has exactly the thirteen variants listed above. **Nothing in the workspace
  classifies any of them as "meaningful progress" today** — that distinction does not exist yet and is
  this sub-slice's central decision.
- 4D's arbitration consumes `no_progress_snapshot_due` as a plain bool. 4G produces the cadence;
  wiring is 4H.
- No warning, epoch, or ordinal machinery exists.

## What this sub-slice does

**1 — Classify progress explicitly and totally.**

Distinguish *meaningful progress* from mere *activity*. The classification must be a **total match over
every `ActivityReason` variant with no `_` wildcard**, so that adding a variant later fails to compile
and forces a decision rather than silently inheriting a default.

**Default to "not progress".** A reason that is not demonstrably progress must be classified as
activity. The asymmetry matters: a spurious warning is harmless (warnings never cancel), whereas a
wrongly-suppressed warning hides a stuck node, which is the failure this cadence exists to surface.
`Heartbeat` is definitively **not** progress — it is the pure-liveness signal.

**Justify the classification in the handoff**: list each variant and which side it falls on, with a
one-line reason. This is a design decision, and it should be reviewable as one.

**2 — The ordinal, exactly as specified.**

`ordinal = floor((now - last_meaningful_progress) / 30m)`, computed from caller-supplied elapsed
milliseconds. Synchronous and clock-free: it reads no clock, spawns nothing, sleeps and selects
nothing.

**3 — Once per ordinal, per epoch.**

Each **positive** ordinal emits exactly once within a progress epoch. Ordinal 0 emits nothing. A
repeated poll at the same ordinal must **not** re-emit — a duplicate wake is expected and must not
duplicate a warning.

**4 — Progress resets the epoch.**

Meaningful progress resets `last_meaningful_progress` and the emitted-ordinal record, so an ordinal
already emitted can legitimately emit again in the **new** epoch. Activity that is not progress resets
neither: it updates only the activity clock.

**5 — Silence never cancels.**

The load-bearing invariant. A warning — at **any** ordinal, however large — must never produce a
cancellation, an impossibility proof, or any other terminal effect. Prove it directly: a test must show
that a very large ordinal still yields a warning and **nothing else**. Assert the absence of the
cancelling outcome, not merely that no error was returned.

## Invariants — must not change

- `crates/bridge-workflow/src/executor.rs` is **untouched**.
- No timer arms; no `select!`, sleep, spawn, token, or cancellation is added or altered.
- Nothing this sub-slice adds can construct a `MechanicalImpossibilityProofV1` (4E) or drive
  cancellation. Elapsed silence is explicitly **not** proof — 4E already tests that; 4G must not
  create a second route to it.
- Readiness ships `Disarmed`; `AutomaticR2f1b` stays unreachable from production.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched. If a change is
  genuinely unavoidable, **stop and report** rather than deciding it silently.

**The refusal gate (decomposition §5).** Re-assert, as 4B–4F did, that no production caller can
construct an automatic attempt while readiness is `Disarmed`.

**Carried from 4B and still binding:** "fully refused" is an *admission-layer* property. Do not add a
second production entry point to `resolve_execution_policy_with_readiness_v1`.

## Out of scope

- Wiring into the executor or the arbitration readiness — 4H.
- Issue #22 closure — 4I. Arming — 4J.
- Changing `ActivityReason`'s variants. Classify what exists; do not add or remove one.

## Required tests

Each must fail on the pre-change tree — verify that, do not assume it:

1. Ordinal boundaries: just under 30m yields 0; **exactly** 30m yields 1; just under 60m yields 1;
   exactly 60m yields 2. Assert the exact boundary, not a value comfortably inside the interval.
2. Ordinal 0 emits nothing.
3. A positive ordinal emits once; a repeated poll at the same ordinal does **not** re-emit.
4. Non-progress activity updates the activity clock and does **not** reset the epoch — the ordinal
   keeps climbing.
5. Meaningful progress resets the epoch, and an already-emitted ordinal emits again in the new epoch.
6. **Silence never cancels**: a very large ordinal yields a warning and no cancellation, no
   impossibility proof, no terminal effect.
7. The classification is total: every `ActivityReason` variant is covered explicitly, with no
   wildcard.
8. The refusal gate, as in 4B–4F.

## Size

**Cap: 350 counted lines** (added nonblank physical Rust lines after `cargo fmt`, docs excluded).
Projection: 250. The cap is a **stop boundary**, not a target. If the change cannot be made within it,
**stop and report** rather than growing it.

## Frozen single-mutation control

Produce a patch reverting exactly one **production** change, record its SHA-256, and verify:

- it applies cleanly to the candidate tree;
- it reddens at least one named test — report the **actual** red population from a **full-suite** run,
  computed as the set difference against the candidate's own pre-existing failures;
- the mutated tree still passes `cargo clippy --all-targets --all-features --locked -- -D warnings`.

Prefer **resetting the epoch on any activity rather than on progress**. That is the exact confusion
this sub-slice exists to prevent — it would let a chatty but stuck node suppress its own warnings
forever — so a control that walks into it is the strongest available.

If the container cannot fetch crates, use the warm cache offline —
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true` with localhost excluded from the injected proxy, and an
explicit `RUSTDOC`. Report doc-test launch failures separately; they are environmental.

## Handoff

Write `docs/superpowers/reviews/2026-08-24-r2f1b-slice4g-handoff.md` covering: what changed, **the
per-variant progress classification with a one-line reason each**, the control patch path and SHA-256,
the actual red population, the deliberate exclusions, and the counted line total against the cap.

**Report gate results truthfully.** If the configured test command is not green, say so and name the
failing test. If a fixture or expectation was hand-written rather than tool-generated, say so. Exclude
diagnostic runs that failed for their own reasons from the gate evidence, and name them.

**Note on the host suite:** nine `tests/smoke_cli.rs` / `tests/fallback_plan_cli.rs` failures are
environmental and intermittent on this lane. If you see them, report them; do not chase them into
production changes.

**Do not record your own head or tree sha.**

## Acceptance Criteria

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`,
  `cargo build --locked`, and the configured test command are all green.
- Every test in "Required tests" exists and fails on the pre-change tree.
- The progress classification is total, explicit, and wildcard-free.
- No warning, at any ordinal, can produce cancellation or an impossibility proof.
- `executor.rs` is byte-identical to the base.
- Counted added nonblank Rust lines ≤ 350.

## Files

- `crates/bridge-core/src/` — the classification and the epoch/ordinal machinery (a new module is fine).
- Test files under `crates/bridge-core/tests/`.

## Spec Refs

- `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` — plan of record.
- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` — §4.3 warning cadence.

## Commit Message

Settle progress epochs and no-progress warning cadence (R2f1b slice 4G)
