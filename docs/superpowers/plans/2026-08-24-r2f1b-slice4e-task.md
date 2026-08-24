---
task-type: implement
---

# R2f1b slice 4E — constructive impossibility proof adapters

## Description

The fifth sub-slice of R2f1b slice 4. It settles **what may and may not count as proof** that a node
can no longer produce a terminal result — the closed list of §4.4, and, just as load-bearing, its
negatives.

The decomposition puts 4E ahead of the lower-impact warning behaviour deliberately: **a false positive
here cancels real work.** Treat every ambiguity as "not proved".

**This sub-slice arms no timer, adds no cancellation path, and changes no scheduling behaviour.**
`crates/bridge-workflow/src/executor.rs` stays byte-identical, including the bare
`let Some(first) = inflight.next().await` that is issue #22. Readiness ships `Disarmed` and
`AutomaticR2f1b` stays unreachable from production.

Base: `origin/main` = `1685aa6c` (R2f1b slice 4D).

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` (§4, sub-slice 4E).
Scope document: `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` §4.4, invariant 2 / D2.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails on the base tree:

- `bridge_core::resource_flight::ResourceIdentityV1` exists with the variants `AcpProcess`,
  `ManagedContainer`, `DedicatedRemoteRequest`.
- `bridge_core::retained_resource_flight` exports `ContainerRemovalObservationV1` and
  `ProcessSignalObservationV1` with the fields named below.
- `crates/bridge-workflow/src/scheduler_arbitration.rs` exports `SchedulerArbitrationReadinessV1`
  carrying a `mechanical_impossibility_proved: bool`.

**Do NOT stop for immaterial measurement differences** — line numbers, diff counts, formatting-only
deltas. Cite by symbol, never by line.

### Verified anchors — operator-measured on this base

- `ResourceIdentityV1::AcpProcess` carries `generation`, `spawn_nonce_sha256`, `pid`, `pgid`,
  `immutable_start: ProcessStartIdentityV1`; `ManagedContainer` carries `generation`, `runtime`,
  `immutable_container_id`, `ownership_labels_digest`.
- `ProcessStartIdentityV1` carries `pid`, `start_time_ticks`, `executable_sha256`.
- `ContainerRemovalObservationV1` carries `immutable_container_id`,
  `observed_noncanonical_a2a_labels`, `removed`, `failure_code`.
- `ProcessSignalObservationV1` carries `pid`, `expected_start_time_ticks`, `signal`, `return_code`,
  `errno`.
- 4D's arbitration consumes `mechanical_impossibility_proved` as a **plain bool readiness input**.
  Nothing produces it today. 4E produces the proof; wiring to the loop is 4H.
- No impossibility vocabulary exists in the workspace beyond 4D's arm name.

## What this sub-slice does

**1 — A proof type that cannot be constructed from a non-proof.**

Introduce a value that means "a terminal result is mechanically impossible". It must be **impossible to
construct except** from one of the three constructive facts below. No `Default`, no public field, no
`from(bool)`, no constructor taking a caller's say-so. If a caller can conjure it, the slice has failed.

**2 — The closed list — exactly three admissible proofs (§4.4).**

- a retained child **exited** while its sole producer result is pending;
- a named container generation is **proved absent after spawn settlement**;
- **all** producer/final routes are irreversibly closed with no terminal result possible.

Each gets an adapter from the real observation types named in the anchors. An adapter returns a proof
**only** when the observation is unambiguous; otherwise it returns "not proved" — never an error that a
caller might unwrap into a proof.

**3 — The negatives, tested as first-class behaviour.**

None of these is proof, and each needs its own test showing **no** proof is produced (§4.4, invariant 2 / D2):

- unknown child state;
- no output;
- elapsed silence;
- file mtime;
- process age;
- provider slowness.

**Ambiguity resolves to "not proved".** In particular: a `ProcessSignalObservationV1` whose
`expected_start_time_ticks` does not match, or whose `errno` leaves the outcome undetermined, is
**not** proof of exit — PID reuse is exactly the confusion this guards. A
`ContainerRemovalObservationV1` with `removed: false`, or with a `failure_code`, or carrying
`observed_noncanonical_a2a_labels`, is **not** proof of absence.

**4 — Feeding arm 6, without wiring it.**

Expose the proof so 4H can populate `mechanical_impossibility_proved`. Do **not** modify
`scheduler_arbitration.rs`'s readiness struct, and do not call it from production.

## Invariants — must not change

- `crates/bridge-workflow/src/executor.rs` is **untouched**.
- No timer arms; no `select!`, sleep, spawn, token, or cancellation is added or altered.
- No production caller cancels anything on the basis of a proof. 4E only decides what a proof *is*.
- Readiness ships `Disarmed`; `AutomaticR2f1b` stays unreachable from production.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched. If a change is
  genuinely unavoidable, **stop and report** rather than deciding it silently.

**The refusal gate (decomposition §5).** Re-assert, as 4B–4D did, that no production caller can
construct an automatic attempt while readiness is `Disarmed`.

**Carried from 4B and still binding:** "fully refused" is an *admission-layer* property. Do not add a
second production entry point to `resolve_execution_policy_with_readiness_v1`.

## Out of scope

- Wiring into the executor or the arbitration readiness — 4H.
- Fixed-grace mechanics — 4F. Progress epochs and warnings — 4G. Issue #22 closure — 4I. Arming — 4J.
- Changing any existing observation type's shape. Adapt what exists; do not redesign it.

## Required tests

Each must fail on the pre-change tree — verify that, do not assume it:

1. Each of the three admissible facts yields a proof, from the real observation types.
2. Each of the six named non-proofs yields **no** proof — six distinct tests, named for the negative.
3. PID-reuse ambiguity: a signal observation whose `expected_start_time_ticks` disagrees is not proof.
4. A container observation with `removed: false`, or with a `failure_code`, or with non-canonical
   ownership labels, is not proof of absence.
5. "All routes closed" requires **all**: a case with one route still open yields no proof.
6. Unrepresentability: prove the proof type cannot be built from a non-proof. Use the repo's
   `trybuild` compile-fail convention — see `crates/bridge-workflow/tests/compile_fail.rs` and
   `crates/bridge-core/tests/compile_fail.rs`. Generate the `.stderr` with `TRYBUILD=overwrite` and
   then verify a clean run; if the environment prevents generation, **say so in the handoff**.
7. The refusal gate, as in 4B–4D.

## Size

**Cap: 400 counted lines** (added nonblank physical Rust lines after `cargo fmt`, docs excluded).
Projection: 280. The cap is a **stop boundary**, not a target. If the change cannot be made within it,
**stop and report** rather than growing it.

## Frozen single-mutation control

Produce a patch reverting exactly one **production** change, record its SHA-256, and verify:

- it applies cleanly to the candidate tree;
- it reddens at least one named test — report the **actual** red population from a **full-suite** run,
  computed as the set difference against the candidate's own pre-existing failures;
- the mutated tree still passes `cargo clippy --all-targets --all-features --locked -- -D warnings`.

Prefer **weakening one negative into a positive** — for example, accepting a signal observation whose
start-time ticks disagree. That is the exact failure mode this sub-slice exists to prevent, so a
control that walks into it is the strongest available.

If the container cannot fetch crates, use the warm cache offline —
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true` with localhost excluded from the injected proxy, and an
explicit `RUSTDOC`. Report doc-test launch failures separately; they are environmental.

## Handoff

Write `docs/superpowers/reviews/2026-08-24-r2f1b-slice4e-handoff.md` covering: what changed, the
control patch path and SHA-256, the actual red population, the deliberate exclusions, and the counted
line total against the cap.

**Report gate results truthfully.** If the configured test command is not green, say so and name the
failing test. If a fixture or expectation was hand-written rather than tool-generated, say that too.
Exclude diagnostic runs that failed for their own reasons from the gate evidence, and name them —
slice 4D did this well and it is the standard here.

**Do not record your own head or tree sha.**

## Acceptance Criteria

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`,
  `cargo build --locked`, and the configured test command are all green.
- Every test in "Required tests" exists and fails on the pre-change tree.
- The proof type is unconstructible from anything but the three admissible facts.
- Every one of the six named non-proofs has its own negative test.
- `executor.rs` is byte-identical to the base.
- Counted added nonblank Rust lines ≤ 400.

## Files

- `crates/bridge-core/src/` — the proof type and its adapters (a new module is fine).
- Test files under `crates/bridge-core/tests/`, plus a compile-fail case.

## Spec Refs

- `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` — plan of record.
- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` — §4.4 closed list, invariant 2 / D2.

## Commit Message

Settle the closed impossibility-proof list and its negatives (R2f1b slice 4E)
