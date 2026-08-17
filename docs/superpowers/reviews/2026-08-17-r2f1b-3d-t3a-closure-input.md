---
task-type: code-review
---

# R2f1b 3d T3a — counted closure on the exact-absence proof

## Description

Review `git diff 1d7826dd..b255cba5` in this checkout — **1,106 changed lines**
across `sweep.rs`, `host_git.rs`, `backend.rs`, one CLI wiring hunk, and a
handoff document.

**What T3a is.** 3d's third task, split in half before dispatch because
(c)+(d) as written does not converge in one review round on this lane. T3a
**DECIDES** and performs **no record mutation**: it builds the state-agnostic
exact-absence proof, the B18 seam bridging the synchronous sweep to the async
registration probe, a fail-closed tri-state refusal, and the recovery-inventory
coupling. **T3b ACTS** — the refusing lock window across proof→transition→unlink,
the `UnusedSettled` transition, descriptor-safe removal, and marker authority.
Do not fault T3a for T3b's absence; do fault it if anything here mutates state
or would not survive T3b acting on its answer.

**The contract.** A candidate may be settled unused only on proof the target
does not exist. The proof is keyed on state-agnostic exact absence so ONE
definition serves both the 2b2 marker population and the candidate population,
and it must add **no edge** to the frozen custody transition table. The tri-state
is fail-closed: target present → refuse; registered-but-absent → refuse;
cannot-prove → refuse; only proven-absent authorizes. A candidate still owned by
a live recovery flight is never provably unused, because T2's recovery inventory
may hold a runner mid-operation.

**History, so you can weigh the evidence.** The first dispatch reached its
3-attempt bound; two rounds of targeted repair followed. Three defects were
found and fixed along the way, and **all three were invisible to the container
pipeline**:

1. An unvalidated legacy sidecar could reach `Authorized`, bypassing the
   `sidecar_file_matches` / `worktree_under_root` guards.
2. `registration_absent` compared paths **byte-exactly** against git's output.
   Git records canonical paths, so any spelling difference — macOS
   `/var` vs `/private/var`, a symlinked root, a trailing slash — read as
   "registration absent" and, with an absent target, **AUTHORIZED**. A
   fail-closed proof failing open. Now fixed with a shared
   `paths_resolve_to_same_identity` comparator.
3. The recovery-owned refusal did not work: `left: Authorized`, expected
   `Refused(CannotProve)`.

**Treat `verify: PASS` as insufficient evidence, and say so if you rely on it.**
The container reported PASS on all four stages for artifacts where the host
found findings 2 and 3. It cannot see them: Linux `/tmp` is not symlinked and
Linux has no `/var → /private/var` indirection.

**Operator evidence — falsifiable, not premise.** On exact `b255cba5`, host,
unloaded: `cargo fmt --all -- --check` clean; `cargo clippy --workspace
--all-targets -- -D warnings` clean; full workspace suite **4,149 passed / 0
failed / 13 ignored across 90 targets**. The non-unix gate is N/A: T3a touches
zero `bridge-core` files. If any claim here does not match the code, report the
mismatch rather than accommodating it.

**Deferred, do not re-litigate unless you find a WRONG:** T3b's whole action
half; the control-root identity defect (its own sub-slice,
`plans/2026-08-16-r2f1b-3d-t2-root-identity-subslice.md`); a reaper
timeout/kill change deliberately reverted as an out-of-scope rider chasing a
container-only flake.

Production reachability: V3 remains unarmed —
`materialize_under_custody` runs only for an admitted `BoundWorktreeCustodyV1`,
which needs a `FrozenR2f1bContractV1`, whose only constructor is inside a
`#[cfg(test)]` module. Latent defects that activate when V3 is armed are still
real findings; rank them, but distinguish them from presently-executable ones.

## Acceptance Criteria

1. Is the tri-state genuinely **fail-closed on every path**? Hunt specifically
   for another way to reach `Authorized` without proof — the class has already
   produced three instances here, so assume a fourth exists until you have
   checked the enumeration.
2. Is `paths_resolve_to_same_identity` correct and used **everywhere** a path is
   compared? Consider unresolvable paths, non-existent paths (the target is
   deliberately gone at proof time), symlinks, case-insensitive filesystems, and
   whether one shared definition is truly shared.
3. Is the recovery-inventory consultation race-free? Name the window if not:
   can a candidate be observed after a recovery entry is half-published, or
   before it becomes visible?
4. Is the B18 seam sound — can it deadlock, block a runtime worker, or make the
   sweep's synchronous contract a lie?
5. Is the path genuinely effect-free? No custody record written, no marker
   removed, no transition-table edge added, on **any** path including error
   paths.
6. Would T3b acting on these decisions be safe — is the decision type
   sufficient, and does it carry what an actor needs to re-verify under a lock?
7. Do the tests discriminate, or do any pass for the wrong reason? This lane has
   shipped false-positive tests before, and one test in this very diff failed
   for its own fixture's reasons.
8. Tag every finding **WRONG** or **SMELL**. WRONG means the code provably does
   the wrong thing — name the input or state and the incorrect result. A finding
   without a concrete failure scenario is a SMELL, never a blocker. WRONG first.
9. End with `VERDICT: APPROVE` or `VERDICT: REJECT` and a one-line SUMMARY.

## Files

- `crates/bridge-worktree/src/sweep.rs` — the proof, the candidate/observation
  types, the decision, the sweep path, the sidecar guards.
- `crates/bridge-worktree/src/host_git.rs` — the probe implementation,
  `paths_resolve_to_same_identity`, the porcelain parsing.
- `crates/bridge-worktree/src/backend.rs` — the recovery-inventory consultation.
- `bin/a2a-bridge/src/main.rs` — boot wiring.

## Spec Refs

- `docs/superpowers/plans/2026-08-17-r2f1b-3d-t3a-task.md` — the T3a contract.
- `docs/superpowers/plans/2026-08-15-r2f1b-3d-dispatch-brief-DRAFT.md` — §3d
  scope and the full T1/T2/T3a execution log.
- `docs/superpowers/reviews/2026-08-17-r2f1b-3d-t3a-handoff.md` — the
  implementor's own account, including its B18 seam design note.
