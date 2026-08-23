---
task-type: implement
---

# T3b slice 5 — boot wiring, legacy markers, and readiness

The last T3b slice. It makes settlement **reachable in production**, extends it to the legacy marker
population, and then — as a **separate commit** — flips the policy gate that has held this whole subsystem
inert since slice B.

Base: `origin/main` = `3d654a0e`.

### Falsification license — scoped to load-bearing anchors

**Stop and report** if a load-bearing anchor is false: a named symbol is absent, a visibility or signature
differs, a described behaviour does not hold, or a requirement cannot be satisfied as written. That
behaviour is wanted, not penalised — it has twice prevented this lane from shipping something wrong.

**Do not stop for immaterial measurement differences.** Counts and totals here are advisory; only the **cap**
is binding. If your count differs from the operator's, record both and continue.

### Anchors, verified at `3d654a0e` by the operator

- `sweep::sweep_orphans(root: &str, my_host: &str, probe: &dyn LeaseProbe)` — note it returns `()` and
  currently discards the report.
- `sweep::sweep_orphans_with_exact_absence` and the private `..._with_pin_opener` variant exist.
- `EXACT_ABSENCE_POLICY_READY_V1` is a `const bool = false` in `sweep/report.rs`, consumed by
  `entry_is_effectively_authorized_for_policy`. **It is the sole remaining production gate.**
- There are **exactly five** `sweep_orphans` call sites, all in `bin/a2a-bridge/src/main.rs`.
- A legacy `*.meta.json` arm exists in `sweep.rs`.
- `WorktreeCustodianV1::replace_unused_settled` (slice 4) supplies `HostGitWorktree` as its probe, and
  `settlement_probe_git_verbs_are_query_only` asserts that path uses only query verbs. **That test must
  continue to pass** — this slice must not introduce a mutating verb into the settlement probe path.

## Part A — boot wiring (first commit)

**`sweep_orphans` stops discarding the report and drives settlement.** Keep its signature unchanged.

**Add `sweep_orphans_async`**, whose entire body offloads the sync sweep:

```rust
pub async fn sweep_orphans_async(root: String, my_host: String, probe: &'static dyn LeaseProbe)
```

implemented as `tokio::task::spawn_blocking(move || sweep_orphans(&root, &my_host, probe)).await`.

**No `async_trait`, no async probe, no new trait.** The probe staying sync is exactly what makes the whole
sweep offloadable as one unit. Repoint all five `main.rs` call sites at the async form.

This addresses the carried closure SMELL *"unbounded sync I/O on async boot paths"*: all five callers are
`async fn`s invoking a sync sweep on the runtime thread, and T3b now adds two `flock` acquisitions, a rename,
an unlink and two `fsync`s per settled record on top of the `git` subprocesses T3a already ran there.

## Part B — the legacy `*.meta.json` population (same commit as A)

Extend settlement to legacy markers **behind the same proof**, the **same two forgery guards**, and the
**same coexistence guard** as the V3 population. Do not introduce a second, weaker path for legacy — a
parallel derivation is how acting and reporting drift apart, which this lane has already paid for.

## Part C — the readiness flip (SEPARATE COMMIT)

Flip `EXACT_ABSENCE_POLICY_READY_V1` to `true` **in its own commit**, with its own frozen control.

This is the moment the subsystem becomes live. It must be independently revertable, so it does not share a
commit with Parts A and B.

Required with it: `readiness_true_still_refuses_a_stale_entry` — proving that readiness alone does not
license acting on a stale report entry. Readiness removes a gate; it does not remove the obligation to
re-open, re-read, re-bind and re-prove under the actor's own lock.

## Required tests

Each must document the production mutation it catches.

1. All five call sites use the async form; a source audit proves no remaining sync `sweep_orphans` call in
   `main.rs`.
2. `sweep_orphans` drives settlement rather than discarding the report.
3. The legacy arm settles only under the same proof and guards as the V3 arm — with a negative per guard.
4. Legacy and V3 markers coexisting on one root behave correctly.
5. `readiness_true_still_refuses_a_stale_entry` (Part C).
6. `settlement_probe_git_verbs_are_query_only` still passes — the destructive path's probe stays query-only.
7. A bounded no-effect audit over any newly added non-settlement path. Follow the amended convention: forbid
   the **mutating** effects by name, assert the added code **originates** no spawn, and make the audit report
   a **named** missing anchor rather than panicking on an `unwrap`.

## Size

Projection **535** counted lines against a cap of **790**. Counted lines are added nonblank physical Rust
lines after `cargo fmt`; a grep for added nonblank lines already excludes blanks — do not subtract them
again. If the projection will exceed the cap, stop before editing and report. Do not delete required tests
to fit.

## Frozen controls — TWO, one per commit

1. Parts A+B: a single-mutation control against that commit, reddening exactly one named test.
2. Part C: its own control against the readiness commit, reddening exactly
   `readiness_true_still_refuses_a_stale_entry`.

Record both paths and both SHA-256 values:
- `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice5-wiring-control.patch`
- `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice5-readiness-control.patch`

## The residual that must not be relaxed

The **stranded `UnusedSettled` marker** (slice 4) remains unclearable by design: the record schema has no
`source` field and `UnusedSettled` forbids the claim that would supply one, so re-proving registration
absence is impossible and the answer is permanently `cannot-prove` → refuse. It is discoverable through the
`CustodyRetirementResidue` operator category.

**This slice must not relax that rule** to make the newly-live sweep tidier: no `source` field, no claim on
`UnusedSettled`, no transition out of it, and no sweep arm that deletes an unprovable marker. If going live
appears to require clearing them, **stop and report** — that is a design question, not an implementation one.

## Handoff

Create `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice5-handoff.md` with the base, the changed-file
list, the counted total against 790, the bounded effect audit, both controls, and an explicit statement that
Part C is a separate commit and independently revertable.

**Do not record this candidate's own head commit or tree sha.** The review loop amends, so any head sha
written inside the handoff is rewritten by the next amend. That binding is the operator's, made in the
evidence commit after the candidate is final.

End the handoff with exactly these six unticked lines:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

Operator note, not a defect: `cargo test --workspace` is red at base on the operator's host with 11
pre-existing `bin/a2a-bridge` failures (`fallback_plan_cli`, `smoke_cli`). The operator compares populations
against the base rather than attributing them.

## Acceptance criteria

- [ ] `sweep_orphans` keeps its signature and drives settlement instead of discarding the report.
- [ ] `sweep_orphans_async` exists, its body is the `spawn_blocking` offload, and all five `main.rs` call
      sites use it. No `async_trait`, no async probe, no new trait.
- [ ] The legacy arm settles only under the same proof, the same two forgery guards, and the same coexistence
      guard as V3, with a negative test per guard.
- [ ] The readiness flip is a **separate commit** with its own frozen control and
      `readiness_true_still_refuses_a_stale_entry`.
- [ ] `settlement_probe_git_verbs_are_query_only` still passes.
- [ ] `LEGAL_CUSTODY_TRANSITIONS_V1` remains ten rows, unchanged.
- [ ] No `source` field, no claim on `UnusedSettled`, no transition out of it, no arm that deletes an
      unprovable marker.
- [ ] Counted lines stay at or under 790.
- [ ] Both frozen controls exist and are SHA-256-recorded, each reddening exactly one named test.
- [ ] The handoff records no head commit or tree sha for this candidate.
- [ ] `Cargo.lock` and every manifest are untouched.
