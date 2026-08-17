---
task-type: code-review
---

# R2f1b 3d T2 — counted re-look on the full unreviewed extension delta

## Description

Review `git diff 435257ce..85658e01` in this checkout — **900 changed lines**
across `crates/bridge-worktree/src/backend.rs`,
`crates/bridge-core/src/fs_custody.rs`, and one handoff document.

**Why the scope is this large.** `435257ce` is the last state any reviewer has
seen; your own prior re-look rejected it with three surviving blockers (below).
The repair that followed (`f66016e0`) was PARKED before any review looked at
it, on evidence that later proved wrong. Two further commits complete it. The
operator considered splitting this into two convergently-sized rounds and the
owner chose one full-scope round instead. Size your effort accordingly: this
delta has never been reviewed.

**What the three commits are:**

- `f66016e0` (736 lines) — the parked convergence-extension repair, built to
  close E1/E2/E3 below.
- `a84c8b57` (107 lines) — targeted repair for one WRONG the operator proved on
  the host (reservation leak on control-root pin failure).
- `85658e01` (70 lines) — operator completion: restores a test the previous
  commit deleted, and fixes a non-recursive `remove_dir` that hung the suite.

**The three blockers you raised on `435257ce`** (verbatim in
`docs/superpowers/reviews/2026-08-16-r2f1b-3d-t2-sol-relook.md`), which this
delta claims to close:

1. **E1** — caller-departure, pre-barrier failure, and runner-exit paths
   published `Failed` without claiming the phase, so a transfer and a failure
   writer could each land a terminal. Claimed fix: a sticky phase CAS with a
   failure arm that every pre-barrier terminal writer must claim.
2. **E2** — `PreparationFlightJournalV1::open` ran before the active flight was
   published, so a stalled filesystem left configure stuck with no owner to
   find or terminalize. Claimed fix: publish the active flight first, pin the
   control root behind it, and move the blocking open off the caller path.
3. **E3** — "replace only exact `Open`" was a TOCTOU claim, not an enforced
   filesystem condition. Claimed fix: an identity-bound, refusing terminal
   replacement using an exact-child lease (this is what the `fs_custody.rs`
   addition serves).

**Operator evidence, offered for checking, not for trust.** Every claim here is
falsifiable — if the code does not match, say so and treat this description as
wrong:

- On exact `85658e01`, host, unloaded: `cargo fmt --all -- --check` clean;
  `cargo clippy --workspace --all-targets -- -D warnings` clean; full workspace
  suite **4,139 passed / 0 failed / 13 ignored across 90 targets**.
- The WRONG that `a84c8b57` closes was proven on the host before the fix: with
  the control-root pin forced to fail, `configure_bound_session` returned the
  typed error but the `preparation_flights` entry was retained, so every later
  configure for that session was refused `AgentOverloaded` for the life of the
  process. Every flight parked on the same failed pin leaked its own
  reservation.
- The restored test in `85658e01` was shown to discriminate: mutating
  `publish_terminal` to skip the durable write fails only that test
  (`left: Some("open")` vs `right: Some("transferred")`), while the four other
  control-root tests still pass.

**Deferred, with prior rulings — do not re-litigate unless you find a WRONG:**
s1 abort residue (abort before first poll / during transfer publication);
the slice-4 binding observer obligation (production observation seam deferred
to slice-4 arming); per-flight blocking waits on the root pin (ruled SMELL);
a test-harness hang amplifier, already ledgered — a panic before
`release_control_root_pin()` turns a clean failure into an unbounded hang
because the hook's condvar wait is unbounded.

Production reachability context: V3 is unarmed, production `claim()` takes no
bound parameter, and no production transfer trigger exists yet. Latent defects
that activate with slice-4 arming are still real findings — say so and rank
them — but distinguish them from presently-executable ones.

## Acceptance Criteria

1. Rule each of **E1, E2, E3** explicitly **FIXED** or **NOT-FIXED** at
   line-numbered source. For any NOT-FIXED, give a constructible schedule or
   state/input and the incorrect result it produces.
2. Review the `a84c8b57` reservation-release fix on its merits: does claiming
   the failure phase before completing, and removing under an `Arc::ptr_eq`
   guard, correctly compose with transfer ownership in **both** race orders?
   Is publishing no durable record the right terminal for that path?
3. Review the `fs_custody.rs` addition as a filesystem-custody primitive —
   whether it actually makes terminal replacement identity-bound and refusing
   rather than advisory.
4. Assess whether the delta introduces any NEW defect, especially in paths the
   green suite does not cover.
5. Tag every finding **WRONG** or **SMELL**. WRONG means the code provably does
   the wrong thing — name the input or state and the incorrect result. A
   finding without a concrete failure scenario is a SMELL, never a blocker.
   Report WRONG items first.
6. Judge the test evidence: do T-A, T-C, the restored stall test, and the
   converted T-B actually discriminate, or do any pass for the wrong reason?
   Call out false-positive tests explicitly — a prior round in this lane
   shipped one.
7. End with `VERDICT: APPROVE` or `VERDICT: REJECT` and a one-line SUMMARY.

## Files

- `crates/bridge-worktree/src/backend.rs` — the preparation flight, its phase
  CAS, the control root, the journal, the runner, transfer, cleanup, and all
  tests.
- `crates/bridge-core/src/fs_custody.rs` — the exact-child lease primitive
  added for E3.

## Spec Refs

- `docs/superpowers/reviews/2026-08-16-r2f1b-3d-t2-sol-relook.md` — your prior
  re-look; E1/E2/E3 are its surviving blockers.
- `docs/superpowers/reviews/2026-08-16-r2f1b-3d-t2-sol-closure.md` — the counted
  closure before it (W1–W4, s1/s2) whose fixes must not have regressed.
- `docs/superpowers/reviews/2026-08-16-r2f1b-3d-t2-handoff.md` — the
  implementor's own account of the design.
- `docs/superpowers/reviews/2026-08-16-r2f1b-3d-t2-extension-repair2-brief.md` —
  this round's declaration, evidence, and ledger.
- `docs/superpowers/plans/2026-08-15-r2f1b-3d-dispatch-brief-DRAFT.md` — §3d
  scope and the T2 execution log.
