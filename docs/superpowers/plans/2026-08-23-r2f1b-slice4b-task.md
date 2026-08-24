---
task-type: implement
---

# R2f1b slice 4B — constructible, fully refused

## Description

The second sub-slice of R2f1b slice 4. It creates the **first production path on which
`DeadlineActivationV2::AutomaticR2f1b` can be produced at all**, and simultaneously proves that no
production caller can reach it yet.

**This sub-slice arms no timer and changes no scheduling behaviour.** The event loop is untouched,
fixed grace stays inactive, and readiness ships **disarmed**. Slice 4J is the arming commit.

Base: `origin/main` = `0ef9b58c` (R2f1b slice 4A).

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` (§4, sub-slice 4B).
Scope document: `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` §2.1, §5.4, §7.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails on the base tree:

- `DeadlineActivationV2` exists in `crates/bridge-core/src/execution_policy.rs` with exactly the
  variants `ManualOnlyR2f1a` and `AutomaticR2f1b`.
- `FrozenWorkflowControlsV1.deadline_activation` is of type `DeadlineActivationV1` and is assigned the
  literal `DeadlineActivationV1::ManualOnlyR2f1a` inside `resolve_execution_policy_v1`.
- `resolve_execution_policy_v1` takes a `PolicyActivationV1` argument, and
  `crates/bridge-workflow/src/admission.rs` calls it with `PolicyActivationV1::Production`.
- `AgentEntry` carries `watchdog: Option<WatchdogConfig>`.
- `workload_fingerprint_with` in `crates/bridge-workflow/src/graph.rs` builds a canonical string from
  the graph and per-agent effective config.

**Do NOT stop for immaterial measurement differences** — line numbers, diff counts, formatting-only
deltas. Cite by symbol, never by line. The scope document's line numbers are known stale.

### Verified anchors — operator-measured on this base

- `AutomaticR2f1b` occurs **exactly once** in the whole workspace: its own variant declaration. No
  caller — production or test — constructs it today.
- `DeadlineActivationV1` has a single variant, `ManualOnlyR2f1a`, and both enums use
  `#[serde(rename_all = "snake_case")]`, so `ManualOnlyR2f1a` has the **same wire bytes** under V1 and V2.
- `resolve_execution_policy_v1` hardcodes `deadline_activation: DeadlineActivationV1::ManualOnlyR2f1a`.
- The only refusal it applies under `PolicyActivationV1::Production` is `FixedGraceInactive` for
  `FanOutPolicyV1::FixedGrace`.
- `WorkflowAdmissionV1::freeze` reads `registry.entry_snapshot(...)` for every node **before** it
  resolves controls, freezes node identities, or touches checkout/provider state. `entry_snapshot` is
  a read-only snapshot, not an effect.
- `FrozenR2f1bContractV1::with_computed_fingerprint(activation, custody_plans)` already accepts a
  `DeadlineActivationV2` — and has **zero callers**.
- `automatic_v3_refuses_legacy_watchdog_before_effects` does not exist yet.

## What this sub-slice does

**1 — A readiness seam, shipped disarmed.**

Add a two-variant `SchedulerActivationReadinessV1 { Disarmed, Armed }` and a
`pub const fn scheduler_activation_readiness_v1() -> SchedulerActivationReadinessV1` returning
`Disarmed`. This function's body is the **entire** thing slice 4J flips; keep it that way. Nothing
else may branch on a build flag, environment variable, or configuration key to reach `Armed`.

**2 — The activation decision, in one place.**

Add one pure function mapping `(SchedulerActivationReadinessV1, PolicyActivationV1)` to a
`DeadlineActivationV2`. It yields `AutomaticR2f1b` **only** for `(Armed, Production)`; every other
pair yields `ManualOnlyR2f1a`. This is the sub-slice's single construction site for `AutomaticR2f1b`.

**3 — Frozen-controls validation admits it.**

`FrozenWorkflowControlsV1.deadline_activation` becomes `DeadlineActivationV2`, carrying the value the
decision function produced rather than a hardcoded literal. `resolve_execution_policy_v1` must
**admit** `AutomaticR2f1b` — no new rejection path — while every existing refusal stands unchanged,
`FixedGraceInactive` included.

*Wire compatibility is load-bearing:* an already-persisted record encoding
`"deadline_activation":"manual_only_r2f1a"` must still decode after the type change, and a
manual-activation record must still encode to the identical bytes. Prove both with a test over
literal bytes, not by round-tripping a freshly built value.

**4 — Activation enters the workload fingerprint.**

`workload_fingerprint` must incorporate the activation, so that an armed run and a manual run of the
same graph never pool into one calibration population (scope document §2.1; that conflation is a
listed BLOCKER). Two graphs identical in every other respect but differing in activation must produce
**different** fingerprints, and the manual-activation fingerprint must be unchanged from the base
tree's — assert that against a literal expected string, so an accidental change to the canonical
encoding cannot pass silently.

**5 — The ACP watchdog refusal (scope document §5.4).**

When the computed activation is `AutomaticR2f1b`, admission refuses if **any** selected agent entry
carries legacy `[agents.watchdog]` settings. The refusal must land after the `entry_snapshot` reads
and **before** control resolution, node-identity freezing, and any checkout, session, or provider
effect. Under `ManualOnlyR2f1a`, a configured watchdog remains exactly as it behaves today — this
path must not change V2 or direct-session behaviour in any way.

Name the test `automatic_v3_refuses_legacy_watchdog_before_effects`, per the scope document's
evidence table.

**6 — The refusal gate (decomposition §5).**

A test asserting that with readiness `Disarmed` — the shipped value — **no** production caller can
obtain `AutomaticR2f1b`, exercised through `WorkflowAdmissionV1::freeze` rather than by calling the
decision function directly. Sub-slices 4B–4I each re-assert this; it is checked every time, not once.

## Invariants — must not change

- No timer arms. No `select!`, sleep, spawn, or cancellation is added or altered.
- `crates/bridge-workflow/src/executor.rs`'s event loop is untouched.
- `FixedGraceInactive` still fires for `FanOutPolicyV1::FixedGrace` under `Production`.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched.
- V2 snapshot and direct-session behaviour is byte-for-byte unchanged.

## Out of scope

- **`contract_fingerprint` into `workload_fingerprint`.** The scope document requires it alongside
  activation, but `FrozenR2f1bContractV1::with_computed_fingerprint` has **zero** production callers on
  this base, so there is no contract fingerprint to incorporate yet. It is deferred to the sub-slice
  that first constructs the contract, and recorded as a carried residual in the handoff. Do not build
  the plumbing here.
- Arming anything. Fixed grace, progress epochs, the multiplexer, and #22 closure are 4C–4J.

## Required tests

Each must fail on the pre-change tree — verify that, do not assume it:

1. `(Armed, Production)` yields `AutomaticR2f1b`; all three other pairs yield `ManualOnlyR2f1a`.
2. Wire compatibility: a literal `"manual_only_r2f1a"` record decodes, and a manual record re-encodes
   to identical bytes.
3. Fingerprint discrimination: same graph, differing activation, different fingerprints — and the
   manual fingerprint equals a literal expected value.
4. `automatic_v3_refuses_legacy_watchdog_before_effects`: refusal occurs with **no** registry
   mutation, session, or provider effect observed. Assert the absence of effects, not merely the
   error.
5. The refusal gate: `Disarmed` admission never yields `AutomaticR2f1b`.
6. A negative for test 4: the same watchdog configuration under `ManualOnlyR2f1a` is **admitted**.

## Size

**Cap: 350 counted lines** (added nonblank physical Rust lines after `cargo fmt`, docs excluded).
Projection: 250. The cap is a stop boundary, not a target. If the change cannot be made within it,
**stop and report** rather than growing it — unused capacity in another sub-slice cannot be
transferred here.

## Frozen single-mutation control

Produce a patch that reverts exactly one production change, record its SHA-256, and verify:

- it applies cleanly to the candidate tree;
- it reddens at least one named test — report the **actual** red population from a **full-suite** run,
  not a filtered one;
- the mutated tree still passes `cargo clippy --all-targets --all-features --locked -- -D warnings`.
  A control that fails on `dead_code` before reaching its red tests proves nothing.

Prefer mutating the activation decision (for example, making `(Disarmed, Production)` yield
`AutomaticR2f1b`) — it should redden the refusal gate and the fingerprint test together, which is
stronger, not weaker.

## Handoff

Write `docs/superpowers/reviews/2026-08-23-r2f1b-slice4b-handoff.md` covering: what changed, the
control patch path and SHA-256, the actual red population, the wire-compatibility evidence, the
deliberate exclusions, and the counted line total against the cap. **Do not record your own head or
tree sha** — that binding is the operator's evidence commit.

## Acceptance Criteria

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`,
  `cargo build --locked`, and the configured test command are all green.
- Every test in "Required tests" exists and fails on the pre-change tree.
- `AutomaticR2f1b` is unreachable from production while readiness is `Disarmed`.
- Persisted manual-activation records are byte-identical before and after.
- Counted added nonblank Rust lines ≤ 350.

## Files

- `crates/bridge-core/src/execution_policy.rs` — readiness enum, decision function, controls field.
- `crates/bridge-workflow/src/admission.rs` — activation wiring and the watchdog refusal.
- `crates/bridge-workflow/src/graph.rs` — fingerprint.
- Test files under `crates/bridge-core/tests/` and `crates/bridge-workflow/tests/`.

## Spec Refs

- `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` — plan of record.
- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` — §2.1 fingerprint, §5.4 watchdog, §7 slices.

## Commit Message

Make AutomaticR2f1b constructible and fully refused (R2f1b slice 4B)
