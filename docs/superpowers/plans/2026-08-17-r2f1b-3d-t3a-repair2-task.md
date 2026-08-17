---
task-type: implement
---

# R2f1b 3d T3a — repair 2: the proof fails OPEN on two paths that must refuse

## Description

Targeted repair on a FROZEN artifact. Base: `5cbfddf2` on branch
`salvage/r2f1b-3d-t3a-repair1-scoped`.

Both findings below were caught by **operator host controls**, not by the
container. The container's `verify: PASS` on all four stages did not hold on the
host: the same two tests that the container passed FAIL there. Take that
seriously — your verify signal is real but it is not sufficient, and neither
finding is hypothetical. Each is a reproduced assertion with exact output.

Do not rework what is delivered and correct: the B18 seam, the tri-state shape,
the sidecar guard from repair 1, effect-freedom, and the `symlink_metadata()`
no-follow probe are all fine. Keep them.

## Finding 1 — WRONG: the registration probe compares paths textually, so it fails OPEN

```
host_git.rs:448  assertion `left == right` failed
  left: BothAbsent
 right: RegisteredButAbsent
```

`registration_absent_sync` (`host_git.rs:148`) hands
`candidate.worktree_path` straight to `registration_absent_from_porcelain`,
which does a **byte-exact** comparison against the path `git worktree list
--porcelain` prints:

```rust
field.strip_prefix(b"worktree ").is_some_and(|path| path == target)
```

Git prints its **canonical** path. The candidate carries whatever spelling it
was constructed with. On macOS a temp path is `/var/folders/…` while git prints
`/private/var/folders/…`, so the comparison never matches. Verified directly
against the host toolchain (git 2.50.1):

```
$ rm -rf ../wt && git worktree list --porcelain
worktree /private/var/folders/…/wt
prunable gitdir file points to non-existent location
```

The registration is plainly still there, and the probe reports it absent.

**Why this is severe, not cosmetic.** An unmatched path means "registration
absent". Combined with an absent target that yields `BothAbsent` — the one
observation that AUTHORIZES settlement. So the failure mode of a proof whose
entire contract is fail-closed is to **fail OPEN**. Any path spelling that
differs textually from git's canonical form triggers it: macOS `/var`, a
symlinked root, a trailing slash, a relative segment.

This is the third instance of raw-versus-canonical path divergence in this lane
(T2's control root was the second, `plans/2026-08-16-r2f1b-3d-t2-root-identity-subslice.md`).

**Required behavior.** Compare paths by resolved identity, not bytes. Canonicalize
both sides — or otherwise compare on a footing that cannot be defeated by
spelling — before deciding a registration is absent. If a side cannot be
resolved, that is **cannot-prove → refuse**, never "absent". Note the async
`registration_absent` shares this comparison; it currently escapes the bug only
because its callers happen to pass canonical paths. Fix the shared definition
rather than adding a second one.

## Finding 2 — WRONG: a recovery-owned candidate is AUTHORIZED

```
backend.rs:11971  assertion `left == right` failed:
  the retained recovery runner owns this otherwise-absent candidate
  left: Authorized
 right: Refused(CannotProve)
```

This is the coupling that made T3 depend on T2 at all: a candidate still owned
by a live recovery flight is not provably unused, however absent its target
looks, because the recovery-owned runner may be mid-operation. The test exists
and asserts the right thing; the production path does not satisfy it.

Note the run also reported:

```
test gate `custody_sync` was never released within 30s — proceeding so the real
failure can surface instead of hanging the suite
```

so the ordering around the custody-sync gate is part of this: before that bound
existed, this test **hung the whole suite** instead of failing. Make the
recovery-ownership consultation correct AND race-free with respect to when the
recovery inventory entry becomes visible; say in the handoff exactly which point
you consult and why that point cannot observe a half-published entry.

## Out of scope

- The reaper timeout/kill change — deliberately reverted in the base commit
  `5cbfddf2`. Do not reintroduce it. It was chasing a container-only flake that
  passes on the host.
- T3b's action half; T1/T2 landed mechanisms; the control-root sub-slice.

## On evidence

Your container has **no compile loop** (implement-lane egress is model APIs
only, ADR-0013), and its `verify` has now been shown to pass tests that fail on
the host. So: **do not present compile errors as red-first evidence, and do not
claim a test passes because verify was green.** State honestly, per test,
whether you ran it. The operator runs the discriminating controls on the host.

**Falsification license.** Both findings are operator claims with reproduced
output attached. If the mechanism does not match the code you read, say so with
evidence rather than forcing a change to fit.

## Acceptance Criteria

1. `host_git::tests::synchronous_exact_absence_capability_distinguishes_all_host_observations`
   passes, including the `RegisteredButAbsent` arm after the target directory is
   removed but before `prune`.
2. A registration whose recorded spelling differs from git's canonical spelling
   (e.g. `/var` vs `/private/var`, a trailing slash, a symlinked parent) is still
   detected as PRESENT — with a test that fails on the base commit.
3. An unresolvable path on either side yields the refusing arm, never `BothAbsent`.
4. `backend::tests::recovery_owned_candidate_refuses_even_when_exact_absence_is_observed`
   passes, and the handoff names the consultation point and why it is race-free.
5. One shared path-comparison definition, not two.
6. The whole `exact_absence` battery is green, and no test hangs.
7. `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --
   -D warnings` clean.
8. `git diff --numstat 5cbfddf2..HEAD` at most 300 changed lines, reported.
9. `crates/bridge-core/src/reaper.rs` is unchanged from the base.

## Files

- `crates/bridge-worktree/src/host_git.rs` — `registration_absent_sync` (`:148`),
  `registration_absent_from_porcelain`, `observe_exact_absence` (`:162`).
- `crates/bridge-worktree/src/backend.rs` — the recovery-ownership consultation.

## Spec Refs

- `docs/superpowers/plans/2026-08-17-r2f1b-3d-t3a-task.md` — the T3a contract.
- `docs/superpowers/plans/2026-08-16-r2f1b-3d-t2-root-identity-subslice.md` —
  the sibling raw-vs-canonical defect, for the shape of the fix.

## Commit Message

Use this exact subject line. Do not wrap it in backticks or a code fence, and do
not copy this instruction sentence into the commit:

fix(3d-t3a): resolve paths before proving a registration absent, and refuse recovery-owned candidates

Body should explain that byte-exact path comparison made the proof fail open
(git prints canonical paths; the candidate carries its original spelling), and
that a candidate owned by a live recovery flight now refuses as cannot-prove.
