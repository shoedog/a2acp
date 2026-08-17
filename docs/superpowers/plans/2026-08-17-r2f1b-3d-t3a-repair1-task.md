---
task-type: implement
---

# R2f1b 3d T3a — targeted repair: the exact-absence sweep must not trust an unvalidated sidecar

## Description

Targeted repair on a FROZEN artifact. Base: `c336d9c7` on branch
`implement/impl-41288-epg2lw7h`. T3a's first dispatch reached its 3-attempt
bound there with `verify` PASS on all four stages (fmt, clippy, build, test).

**Do not rework what is already delivered and correct.** Operator-verified at
source on `c336d9c7`:

- The B18 seam, `ExactAbsenceProbeV1`, and the tri-state refusal are in place.
- The recovery-inventory coupling is implemented
  (`decide_unused_candidate_for_recovery`, `backend.rs:2271`) with a test
  (`recovery_owned_candidate_refuses_even_when_exact_absence_is_observed`).
- Attempt 2's symlink defect is FIXED: `target_absent_from_probe`
  (`host_git.rs:108`) uses `symlink_metadata()`, so a dangling symlink reads as
  present, not absent. Keep it that way.
- The path is effect-free: no custody record is written, no marker removed, no
  transition-table edge added.
- 699 changed lines, inside the 750 cap.

This repair closes exactly ONE finding.

## The finding — WRONG

`sweep_orphans_with_exact_absence` (`crates/bridge-worktree/src/sweep.rs:460`)
takes a `ScannedWorktreeRecordV1::Legacy(sidecar)` and builds its candidate
directly from `sidecar.canonical_source` and `sidecar.worktree_path`:

```rust
ScannedWorktreeRecordV1::Legacy(sidecar) => {
    let marker = ExactAbsenceCandidateV1::new(
        sidecar.canonical_source.as_str(),
        sidecar.worktree_path.as_str(),
    );
    let decision = decide_unused_marker(&marker, exact_absence_probe);
```

It never applies the validation `sweep_orphans` applies to the same records —
the `sidecar_file_matches` and `worktree_under_root` guards. So a forged or
stale `*.meta.json` naming an out-of-root `worktree_path` that happens to be
absent walks straight to `Authorized`.

That contradicts the task's fail-closed contract: an unvalidated sidecar is not
proof of anything, and "cannot prove" must refuse. It matters even though the
path is effect-free, because the decision value IS T3a's deliverable — T3b will
act on exactly this answer.

**Required behavior.** Apply the same validation `sweep_orphans` already uses on
a legacy sidecar, before the record is eligible to produce any decision. A
sidecar that fails validation yields the **refusing** arm of the tri-state
(cannot-prove), never `Authorized`. Reuse the existing guards — do not write a
second definition of "this sidecar is trustworthy", because a second definition
is a second place for it to weaken.

## Red-first tests (required)

- A forged sidecar naming an out-of-root, absent `worktree_path` yields the
  refusing arm, not `Authorized`.
- A sidecar whose file does not match yields the refusing arm.
- A valid, in-root sidecar whose target and registration are both provably
  absent still yields `Authorized` — the guard must not refuse everything.

## Out of scope

Everything else in `c336d9c7`; the T3b action half; the control-root identity
sub-slice; T1's and T2's landed mechanisms.

## On evidence — read this, it changes what you must produce

Your container's egress permits model APIs only; crates.io is absent by design
(ADR-0013), so you have **no local compile loop** and `cargo` will 403. Do not
try to fix that, and never weaken or delete a test because you could not run it.

Consequently: **do not fabricate red-first evidence, and do not present
compilation errors as red-first proof.** The previous attempt's handoff listed
`error[E0425]: cannot find function ...` as its red-first output. A compile
error proves only that an API did not exist yet; it is not evidence that a test
discriminates behavior. If you cannot run a test, say exactly that — "not run,
no local toolchain" — and describe the mutation that SHOULD redden it. The
operator runs the discriminating red/green controls on the host. An honest "not
run" is worth more than a misleading transcript.

**Falsification license.** Every claim above is an operator claim. If the code
does not match it, say so with evidence rather than forcing the change.

## Acceptance Criteria

1. An unvalidated or out-of-root legacy sidecar can never produce `Authorized`;
   it takes the refusing arm.
2. The existing `sweep_orphans` guards are reused, not reimplemented.
3. A valid in-root sidecar with proven absence still reaches `Authorized`.
4. The three tests above exist and pass.
5. Nothing else in the artifact changes behavior; the path stays effect-free and
   adds no transition-table edge.
6. `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --
   -D warnings` clean; workspace suite green.
7. `git diff --numstat c336d9c7..HEAD` at most 200 changed lines, reported in
   the handoff.
8. The handoff states honestly, per test, whether it was executed or not.

## Files

- `crates/bridge-worktree/src/sweep.rs` — the T3a sweep path and the existing
  legacy-sidecar guards it must reuse.

## Spec Refs

- `docs/superpowers/plans/2026-08-17-r2f1b-3d-t3a-task.md` — the T3a contract.
- `docs/superpowers/reviews/2026-08-17-r2f1b-3d-t3a-handoff.md` — the first
  dispatch's own account.

## Commit Message

Use this exact subject line, with no surrounding code fence or backticks:

fix(3d-t3a): refuse an unvalidated legacy sidecar instead of authorizing it

Then a body explaining that the exact-absence sweep reused the existing
sidecar_file_matches / worktree_under_root guards so a forged or stale
`*.meta.json` naming an out-of-root absent path takes the refusing arm rather
than reaching Authorized, and that a valid in-root sidecar still authorizes.
