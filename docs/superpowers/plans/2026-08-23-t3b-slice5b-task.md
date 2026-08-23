---
task-type: implement
---

# T3b slice 5B — the readiness flip

## Description

The final T3b increment, and the smallest. It flips one constant from `false` to `true`, arming a subsystem
that has been inert since slice B, and proves that arming it removes a **gate** without removing an
**obligation**.

Base: `origin/main` = `d6b3bb4d`.

Keep this slice tiny. Its value is that it is independently revertable: a single reviewable commit whose
revert disarms the subsystem completely. Do not fold unrelated cleanup, refactoring or "while we're here"
changes into it.

### Falsification license — scoped to load-bearing anchors

Stop and report if a load-bearing anchor is false: a named symbol absent, a signature different, a
described behaviour that does not hold, or a requirement that cannot be satisfied as written. Do NOT stop
for immaterial measurement differences; counts are advisory and only the cap binds. The clone's
`origin/main` ref may lag — `git rev-parse HEAD` is authoritative.

### Anchors, verified at `d6b3bb4d` by the operator

- `crates/bridge-worktree/src/sweep/report.rs` declares `const EXACT_ABSENCE_POLICY_READY_V1: bool = false;`
  and consumes it in `entry_is_effectively_authorized_for_policy`. **It is the sole remaining production
  gate.**
- `sweep_orphans` drives settlement (5A) and `sweep_orphans_async` exists with five `main.rs` call sites.
- `LEGAL_CUSTODY_TRANSITIONS_V1` has ten rows and is frozen.
- `settle::reprove_under_window` re-proves exact absence under a held window; the report carries ordered
  historical evidence, not authority.

## What this slice does

**Flip `EXACT_ABSENCE_POLICY_READY_V1` to `true`.** That is the production change. It should be one line.

## The required test — the point of the whole slice

`readiness_true_still_refuses_a_stale_entry`.

Readiness removes a **gate**; it does not remove the **obligation** to re-open, re-read, re-bind and
re-prove under the actor's own lock. The test must prove that with readiness `true`, a report entry that
was authoritative when the report was taken is still refused when the world changed underneath it — for
example the target reappearing between the report and the window.

Construct it so it would **fail if readiness alone were treated as sufficient authority**. A test that
merely passes with readiness true, without a stale entry to refuse, does not discharge this requirement.

## Additional required checks

- The ten-row `LEGAL_CUSTODY_TRANSITIONS_V1` is unchanged.
- No `source` field on the record, no claim permitted on `UnusedSettled`, no transition out of it, and no
  sweep arm that deletes an unprovable marker. **Going live must not be made tidier by relaxing the
  stranded-marker rule.** If arming the subsystem appears to require clearing stranded markers, **stop and
  report** — that is a design question, not an implementation one.
- `settlement_probe_git_verbs_are_query_only` still passes.

## Size

Projection **90** counted lines against a cap of **200**. Counted lines are added nonblank physical Rust
lines after `cargo fmt`; a grep for added nonblank lines already excludes blanks. If the projection will
exceed the cap, stop before editing and report — a large diff here means the slice has been
misunderstood.

## Frozen control

Freeze a single-mutation control against this slice's own head at
`docs/superpowers/reviews/2026-08-23-r2f1b-t3b-slice5b-readiness-control.patch`, chosen so that removing it
defeats the re-prove obligation — for example, accepting the report entry's authority without re-proving
under the window. It must redden **exactly one** named test, and that test should be
`readiness_true_still_refuses_a_stale_entry`. Record its SHA-256.

## Handoff

Create `docs/superpowers/reviews/2026-08-23-r2f1b-t3b-slice5b-handoff.md` with the base, the changed-file
list, the counted total against 200, the frozen control's path and SHA-256, and an explicit statement that
this commit is the arming point and that reverting it disarms the subsystem.

**Do not record this candidate's own head commit or tree sha.** The review loop amends, so any head sha
written inside the handoff is rewritten by the next amend. That binding is the operator's.

End the handoff with exactly these six unticked lines:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

Operator note, not a defect: `cargo test --workspace` is red at base on the operator's host with 11
pre-existing `bin/a2a-bridge` failures. The operator compares populations against the base rather than
attributing them, and has seen that population inflate under parallel load — it is compared on an idle
machine.

## Acceptance criteria

- [ ] `EXACT_ABSENCE_POLICY_READY_V1` is `true`.
- [ ] `readiness_true_still_refuses_a_stale_entry` exists, passes, and would fail if readiness alone were
      treated as sufficient authority.
- [ ] `LEGAL_CUSTODY_TRANSITIONS_V1` is still ten rows, unchanged.
- [ ] No `source` field, no claim on `UnusedSettled`, no transition out of it, no arm deleting an
      unprovable marker.
- [ ] `settlement_probe_git_verbs_are_query_only` still passes.
- [ ] Counted lines stay at or under 200.
- [ ] The frozen control exists, is SHA-256-recorded, and reddens exactly one named test.
- [ ] The handoff records no head commit or tree sha for this candidate.
- [ ] `Cargo.lock` and every manifest are untouched.
