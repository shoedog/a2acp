---
task-type: implement
---

# Path-identity primitive — repair 3: bracket the whole resolution, not just the inode

## Description

Targeted repair on a reviewed artifact. Base: `39f8c3e1` on branch
`salvage/r2f1b-path-identity-repair2`, which is based on current `main`.

The artifact is **green on the host** (`fmt` 0, `clippy --workspace --all-targets
-D warnings` 0 with zero warning lines, `cargo test --workspace --locked
--no-fail-fast` 0 — 4,157 passed / 0 failed / 13 ignored across 91 binaries) and
a counted closure confirmed **six of the seven** repaired defects closed. This is
a small, precisely-specified repair of the seventh plus three test-evidence gaps.

**Do not redesign anything.** The comparison rule (A1–A8, reproduced below) is
pinned and was confirmed by the closure to be implemented correctly. This task
changes *when a computed verdict may be trusted*, not what the verdict is.

### The pinned rule, unchanged and NOT up for revision

| # | Condition | Verdict |
|---|---|---|
| A1 | Both paths exist | device+inode identity ⇒ `Same` / `Different` |
| A2 | Deepest existing ancestors are different objects | `Different` |
| A3 | Same ancestor; missing tails differ in component count | `Different` |
| A4 | Same ancestor; missing tails byte-equal | `Same` |
| A5 | Same ancestor; **any** differing pair pure-ASCII both sides and **not** ASCII-casefold-equal | `Different`, probe NOT consulted |
| A6 | Same ancestor; no A5 pair, and **any** differing pair has a non-ASCII byte | `CannotProve`, both case branches, unconditionally |
| A7 | Same ancestor; every differing pair pure-ASCII and ASCII-casefold-equal | sensitive ⇒ `Different`; insensitive ⇒ `CannotProve`; undeterminable ⇒ `CannotProve` |
| A8 | Unresolvable, or **any drift during the comparison** | `CannotProve`, never `Different` |

**Asymmetric fix authority still applies.** A wrong `Different` is fail-open; a
wrong `CannotProve` merely refuses. Every change below narrows `Different` to
`CannotProve`. **Do not widen `Different` anywhere in this task.**

## W1 — BLOCKER: the stability bracket ignores resolved-tail drift

`ancestors_are_stable_with_resolver` (`crates/bridge-core/src/fs_custody.rs`,
~line 1717) re-resolves both paths but compares **only** `identity`. It ignores
the resolved `canonical` path and the `missing_tail`. A verdict computed from the
first resolution is therefore returned even when the second resolution describes
a *different* path topology.

**Constructible state — this is the failure, verified against the code:**

- `/R` exists; `/R/link` and `/R/foo` are both absent.
- Compare `/R/link/foo` against `/R/foo`.
  - First resolution: both get deepest existing ancestor `/R`. Tails are
    `["link", "foo"]` and `["foo"]`.
  - Component counts differ ⇒ **A3 ⇒ `Different`**.
- Before the stability check runs, create the symlink `/R/link -> /R`.
- Second resolution: `/R/link/foo` now has deepest existing ancestor `/R/link`,
  which canonicalizes to `/R` with tail `["foo"]`. `/R/foo` is unchanged:
  ancestor `/R`, tail `["foo"]`.
  - **Both identities are unchanged**, because `/R/link` *is* `/R`.
- The bracket passes and the stale `Different` is returned — but the two paths
  now **alias**. The correct answer under drift is `CannotProve`.

Production reachability: a porcelain registration spelled the first way is then
discarded as unrelated in `registration_absent_from_porcelain`, yielding `Absent`
⇒ `BothAbsent` ⇒ `Authorized` in the sweep. That is fail-open exact-absence
evidence and possible custody loss. Trigger is rare — it needs a stale nested
registration plus concurrent symlink creation — but worktree paths are
operator-supplied and arbitrary.

**Fix:** compare the **complete** second `DeepestExistingPathV1` snapshot against
the first — `identity`, `canonical`, **and** `missing_tail` — for both sides. Any
difference ⇒ `CannotProve`. Deriving `PartialEq` on the snapshot type (or writing
an explicit field-wise comparison) is in scope; if you derive it, make sure every
field is compared and none is skipped.

Keep the existing disclosure that this is string-path bracketing, not descriptor
binding: an ABA replacement restoring an identical snapshot before re-resolution
is still outside the proof. Widening the bracket does not change that, and the
handoff must not start claiming ABA safety.

**Red-first tests** (each must fail on `39f8c3e1`):

- The symlink case above, driven through the deterministic resolver seam:
  identities unchanged, `missing_tail` changed ⇒ `CannotProve`, **not**
  `Different`.
- A canonical-path-changed variant: identity and tail unchanged, `canonical`
  changed ⇒ `CannotProve`.
- **A stable control**: nothing drifts ⇒ the computed verdict survives unchanged.
  Without this, a mutant that always returns `CannotProve` passes the other two.

## S1 — the B4 common-dir barrier can pass for the wrong reason

The swap hook runs **after** `spawn`, so Git may read `.git` or finish before the
swap lands, and the two renames expose a brief interval with no `.git` at all.
The hook branch also spawns without piped stdout/stderr, and the test asserts only
`.is_err()` — so an ordinary Git failure satisfies it just as well as the
post-command revalidation firing.

**Fix:** move the seam to fire **after the initial revalidation and before the
spawn** so the swap is guaranteed to be in place for the observation; keep stdout
and stderr piped; and assert the **specific** revalidation error rather than any
error. The production post-check itself is correct and stays as it is — this is
test evidence only.

## S2 — the B5 end-to-end fixture cannot distinguish A6 from a Git failure

`host_git_ambiguous_registration_publishes_registration_unproven` corrupts `HEAD`,
then asserts the persisted locator is `RegistrationUnproven`. But both
`Ok(CannotProve)` **and** any `worktree list` `Err` map to `RegistrationUnproven`,
and the test never asserts the porcelain call succeeded or that its output
contains the stale registration. Across Git versions the test could pass while
proving nothing about A6.

This is the same family as the fixture defect already repaired in this artifact
once — a test that looks like evidence and is not. Treat it accordingly.

**Fix:** assert that the porcelain invocation **succeeds** and that its output
contains the exact stale path, and — preferably — assert the parser's own
`CannotProve` result directly, so the durable-record assertion is a second
independent check rather than the only one.

## S3 — B7 has no unchanged-sample positive control

The deleted-sample and replaced-sample tests both expect `None`. A mutant making
`sampled_entry_still_matches` return `None` unconditionally passes both, and the
B3 test bypasses the real probe by injection. The failure that would slip through
is fail-closed A7 over-refusal, not a fail-open — so this is a SMELL, not a
blocker — but it is a two-line control.

**Fix:** add a `#[cfg(unix)]` test asserting that an **unchanged** ASCII sample
yields `Some(_)` from the real probe.

## On evidence

Your container has **no compile loop**, and Linux has neither a case-insensitive
filesystem nor the macOS `/var`→`/private/var` indirection — the rows that matter
most here are the ones your container cannot run. Do not present a green verify as
evidence that a test passes. State **per test** whether you executed it. The
operator runs the discriminating gates on the host.

`bridge-core` compiles for Windows in CI while `liveness` and
`namespace_transaction` are `#[cfg(unix)]`. Do not reference those from
`fs_custody` without a `#[cfg(unix)]` guard on the referencing item, and gate
anything that becomes unused on non-unix with
`#[cfg_attr(not(unix), allow(dead_code))]` — the lane is warning-clean under
`-D warnings`. The established shape is commit `790b4191`.

## Acceptance Criteria

1. W1 is fixed: the stability bracket compares the complete resolution snapshot —
   identity, canonical path and missing tail — on both sides, and any drift yields
   `CannotProve`.
2. The three W1 red-first tests exist and pass, **including the stable control**.
3. S1, S2 and S3 are fixed as described; each is test-only except S1's seam
   placement.
4. No production behavior changes other than W1 narrowing `Different` to
   `CannotProve` under drift. **`Different` is not widened anywhere.**
5. The A1–A8 table is untouched. No new dependency.
6. The handoff still discloses the non-ABA limitation and does **not** claim ABA
   safety, and its per-row execution table is extended to the new tests with an
   honest executed / not-executed mark for each.
7. `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
   --locked -- -D warnings` clean; `cargo test --workspace --locked
   --no-fail-fast` green with totals reported.
8. `git diff --numstat 39f8c3e1..HEAD` at most **250** changed lines. This repair
   is small and precisely specified; a breach needs an explicit operator waiver.

## Files

- `crates/bridge-core/src/fs_custody.rs` — the stability bracket and its resolver-seam tests.
- `crates/bridge-worktree/src/host_git.rs` — the B4 barrier seam and its test.
- `crates/bridge-worktree/src/backend.rs` — the B5 end-to-end fixture.
- `docs/superpowers/reviews/2026-08-17-r2f1b-3d-t3a-path-identity-handoff.md` — the artifact handoff to extend.

## Spec Refs

The pinned rule and the failure states are reproduced in full above. The
supporting records live on a planning branch and are **not in your checkout**;
their absence is not a missing input and is not a reason to pause.

## Commit Message

fix(fs-custody): bracket the whole resolution, not just the ancestor inode

The stability re-check compared only ancestor (dev, ino), so a verdict computed
from the first resolution survived even when the second described a different path
topology. Comparing /R/link/foo with /R/foo gives A3 Different from tails
[link, foo] versus [foo]; creating link -> /R before revalidation makes both
re-resolve to ancestor /R with tail [foo] while leaving both identities unchanged,
so the bracket passed and returned a stale Different for two paths that now alias.
Fail-open in a fail-closed proof.

The bracket now compares the complete resolution snapshot on both sides — identity,
canonical path and missing tail — and any drift refuses. This only narrows
Different to CannotProve; it widens nothing. The check remains string-path
bracketing rather than descriptor binding, and an ABA replacement that restores an
identical snapshot is still outside the proof.

Three test-evidence gaps go with it. The common-dir barrier fired after spawn and
asserted only is_err, so an ordinary Git failure satisfied it; the seam now fires
before spawn, keeps output piped, and asserts the specific revalidation error. The
durable-record fixture could not distinguish A6 ambiguity from a worktree-list
failure, since both map to RegistrationUnproven; it now asserts the porcelain
succeeded and carried the stale registration. And the sampled-entry revalidation
gained the unchanged-sample control that its two negative tests could not provide.
