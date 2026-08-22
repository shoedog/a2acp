---
task-type: implement
---

# T3b slice 4 — candidate settlement (the destructive slice)

## Description

This is the first T3b slice that **mutates custody state and unlinks**. Proof, transition and marker
retirement happen inside one held window, over an edge that is already frozen.

**Marker only.** No `git`, no `prune`, no `remove_dir_all`, no process spawn originating in the added code.
The checkout directory is never touched — only the custody marker is.

Base: `origin/main` = `c343e563`.

### Falsification license

Every claim below is a tripwire. If any anchor is false — a symbol absent, a visibility different, a
signature other than stated — **stop and report it**. Do not adapt around a false anchor. In this lane the
source plan has already been wrong three times, once about a safety property, so verify rather than trust.

### Anchors, verified at `c343e563` by the operator

- `custody::LEGAL_CUSTODY_TRANSITIONS_V1` is a `&[(WorktreeCustodyStateKindV1, WorktreeCustodyStateKindV1)]`
  containing **exactly ten** pairs. Its **first** row is `(ProtectionPrepared, UnusedSettled)` — the edge this
  slice uses. **The edge already exists; do not add it.** Because the constant is a slice and not a fixed-size
  array, assert `.len() == 10`.
- `WorktreeCustodyRecordV1` has exactly these fields: `schema_version`, `custody_id`,
  `checkout_fingerprint`, `current_attempt`, `worktree`, `state`, `claim`. **There is no `source` field.**
- `WorktreeCustodyStateV1::UnusedSettled {}` maps to `ClaimPresenceV1::Forbidden`.
- `custody_writer` has a private method `stage_and_settle(&self, record: &WorktreeCustodyRecordV1,
  mode: PublicationModeV1) -> Result<(), CustodyWriteRefusalV1>`.
- `WorktreeCustodianV1` exists and is used in `backend.rs`.
- `settle::reprove_under_window` and `settle::SettlementWindowV1` exist (T3b slices 1–2).
- `bridge_core::fs_custody::retire_captured_regular_child_v2` exists (T3b slice 3) and has **no**
  `bridge-worktree` caller yet.

### Correction to the plan's carry-forward — this obligation lands HERE, not on slice 5

The plan assigns the read-only-probe obligation to slice 5. **That is one slice too late.** The operator
confirmed at `c343e563` that `reprove_under_window` still has **no production caller** — the only mention
outside its own module is a doc comment in `sweep.rs`. This slice introduces the first one.

`reprove_under_window` takes `probe: &dyn ExactAbsenceProbeV1`. The production implementor `HostGitWorktree`
spawns `git rev-parse`. **The probe this slice supplies must be read-only and must not spawn a process from
the settlement path.** State in the handoff which probe is supplied and why it is read-only. If the only
available production probe spawns, stop and report rather than wiring it.

## What this slice builds

**`WorktreeCustodianV1::replace_unused_settled`** over the already-frozen edge.

**One shared publication derivation.** Extract the body of `custody_writer`'s `stage_and_settle` into a free
`publish_custody_record_in(pin, name, record, mode)` so the settler and the custodian share **one**
derivation rather than two. Two independent publication paths is precisely how the acting and reporting paths
drift apart, and this lane has already paid for that once.

**The settle sequence**, entirely inside one held window: re-prove under the window → transition
`ProtectionPrepared → UnusedSettled` → retire the marker via slice 3's primitive → parent sync.

## The stranded-marker residual — carry it, never relax it

A crash **between** the transition and the retirement leaves a durable `UnusedSettled` record that **no later
sweep can authorize removing**. The operator verified the mechanism: the record schema carries no `source`,
and `UnusedSettled` forbids the claim that would supply one, so re-proving registration absence is impossible
and the tri-state answer is `cannot-prove` → **refuse**.

This is correct and fail-closed. It is also a real, bounded leak.

- This slice **must** emit a **distinct, operator-visible category** for a stranded `UnusedSettled` marker, so
  an operator can find them without inferring.
- **No slice may relax the rule to clear it.** Do not add a `source` field, do not permit a claim on
  `UnusedSettled`, and do not add a transition out of it. If clearing it seems necessary, stop and report.

## Required tests

Each must document the production mutation it catches.

1. `unused_candidate_settles_only_after_exact_absence`, covering all three arms:
   - target **present** → refuses;
   - **registered but absent** → refuses;
   - **both absent** → settles, **marker only, checkout directory untouched**.
2. `a_materialization_in_flight_candidate_is_never_settled`.
3. `the_frozen_transition_table_is_unchanged` — asserts `LEGAL_CUSTODY_TRANSITIONS_V1.len() == 10` **and**
   that the ten pairs are exactly as they are today.
4. A crash between transition and retirement leaves a **durable** `UnusedSettled` and **loses nothing** — the
   checkout survives, the residue is recognizable, and the stranded category is reported.
5. The settlement path supplies a read-only probe and spawns no process.
6. A bounded no-effect audit proving the added path reaches no `git`, `prune`, `remove_dir_all`, or process
   spawn. Follow slice 2's amended convention: forbid the **mutating** effects by name, assert the added code
   **originates** no spawn, and make the audit report a **named** missing anchor rather than panicking on an
   `unwrap` — slice 2 lost a round to exactly that.
7. Settlement is refused when the window is not held.

## Size

Projection **535** counted lines against a cap of **790**. Counted lines are added nonblank physical Rust
lines after `cargo fmt`. A grep for added nonblank lines already excludes blanks — do not subtract them
again. If the projection will exceed the cap, stop before editing and report a revised estimate. Do not
delete required tests to fit.

## Frozen control

Freeze a **single-mutation control against this slice's own head** at
`docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice4-mutation-control.patch`. One logical mutation chosen so
removing it defeats the settlement precondition — for example, settle without requiring the re-proof to have
succeeded. It must redden **exactly one** named test. Record its SHA-256.

## Handoff

Create `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice4-handoff.md` with the base, changed-file list,
counted total against the 790 cap, the bounded effect audit, the probe statement, the stranded-marker
category, and the control's path, SHA-256, mutation and single reddening test.

**Do not record this candidate's own head commit or tree sha.** The review loop amends, so any head sha
written inside the handoff is rewritten by the next amend and becomes unreachable. That binding is the
operator's, made in the evidence commit after the candidate is final.

End the handoff with exactly these six unticked lines:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

Note for the operator, not a defect to fix: `cargo test --workspace` is **red at base** on this host with 11
pre-existing failures in `bin/a2a-bridge` (`fallback_plan_cli`, `smoke_cli`), which are host
system-integration tests the verify configuration documents as unrunnable hermetically. Report the population
and compare it against the base rather than treating it as this slice's regression.

## Acceptance criteria

- [ ] `LEGAL_CUSTODY_TRANSITIONS_V1` is unchanged, still ten rows, asserted by a test.
- [ ] No new transition edge is added, and no transition **out of** `UnusedSettled` exists.
- [ ] No `source` field is added to the record, and `UnusedSettled` still forbids a claim.
- [ ] Settlement retires the **marker only**; a test proves the checkout directory is untouched.
- [ ] The added path reaches no `git`, `prune`, `remove_dir_all`, or process spawn, proved by an audit that
      names a missing anchor instead of panicking.
- [ ] The settlement path supplies a **read-only** probe, and the handoff says which and why.
- [ ] A crash between transition and retirement is covered by a test and surfaces a distinct
      operator-visible stranded-marker category.
- [ ] `stage_and_settle`'s body is extracted so settler and custodian share **one** publication derivation.
- [ ] Counted lines stay at or under 790.
- [ ] The frozen control exists, is SHA-256-recorded, and names exactly one test that must redden.
- [ ] The handoff records no head commit or tree sha for this candidate.
- [ ] `Cargo.lock` and every manifest are untouched.
