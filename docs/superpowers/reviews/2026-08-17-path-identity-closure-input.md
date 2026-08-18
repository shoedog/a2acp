---
task-type: code-review
---

# Path-identity primitive — counted closure

## Description

Review `git diff 227c8ecc..be7c6708` in this checkout — **852 changed lines**
across `crates/bridge-core/src/fs_custody.rs` (the primitive),
`crates/bridge-worktree/src/host_git.rs` and `.../sweep.rs` (the migrated
callers), plus a handoff.

**What this is.** One shared path-identity primitive for the worktree lane,
built because the lane got the same thing wrong **five times** across two
slices — a path compared by *spelling* where *identity* was required — and
three of those failed OPEN in proofs whose contract is fail-closed. Four
targeted repairs had patched instances in place; a fifth appeared anyway, so the
class was escalated to a designed primitive.

**The contract.** Three answers: `Same`, `Different`, `CannotProve`. Only a
**proven** `Different` may let a caller skip or dismiss something; `CannotProve`
always refuses. Previous implementations collapsed "not provably equal" into
"different", which is exactly what made them fail open.

**The hard part, and the rule adopted.** Non-existent paths are the normal case
here — the exact-absence proof asks about a target that is *supposed* to be
gone, so neither side can be stat'd. The rule resolves each path's deepest
existing ancestor; different ancestor objects prove difference; a shared
ancestor defers to the filesystem's own name semantics. For the missing tail:

| Condition | Result |
|---|---|
| byte-equal | `Same` |
| case-sensitive ancestor | bytes decide ⇒ `Different` |
| ASCII-case-fold-equal | `CannotProve` |
| both names pure ASCII | `Different` — normalization cannot relate them |
| one name's ASCII-letter skeleton is a subsequence of the other's | `CannotProve` |
| otherwise | `Different` |

The skeleton test rests on: canonical decomposition only ever ADDS ASCII base
letters (NFC `é` decomposes to ASCII `e` plus a combining acute) and never
removes one, so canonical equivalents must satisfy the subsequence relation.
Failing it is therefore a *proof* of difference, with no Unicode tables and **no
new dependency** — `Cargo.toml` and `Cargo.lock` are byte-identical to base.

**This rule was wrong twice before it was right, and both corrections came from
running things rather than reasoning.** Weigh it accordingly:

1. A blanket refusal on any non-ASCII byte was sound but inert — one non-ASCII
   name anywhere in a repository would stop the proof authorizing at all.
   Counterexamples that forced the change: `équipe` vs `other`, `éa` vs `éb`,
   and `["wt","child-a"]` vs `["WT","child-b"]`.
2. The subsequence test then wrongly applied to pure-ASCII names, so `w` and
   `wt` compared as possibly-equivalent (`w`'s skeleton IS a subsequence of
   `wt`'s). That broke the pre-existing
   `porcelain_registration_check_is_exact_and_handles_locked_records`.
3. The comparator also returned `CannotProve` at the FIRST ambiguous component,
   so a provably-different later component never settled the path.

**Operator evidence — falsifiable, not premise.** On exact `be7c6708`, host,
unloaded: fmt clean; `cargo clippy --workspace --all-targets -- -D warnings`
clean; full suite **4,147 passed / 0 failed / 13 ignored across 91 targets**.
Red-first: the both-directions table fails on the pre-change comparator at
"non-ASCII vs unrelated ASCII: must be provably different". There is **no local
non-unix gate** (one was attempted and withdrawn), so the `cfg(unix)` boundary
in `bridge-core` was checked by reasoning only — the change is pure string/slice
logic referencing no `cfg(unix)` module. Say so if you disagree.

## Acceptance Criteria

1. **Is the rule sound in the `Different` direction?** A wrong `Different` is a
   fail-open, which is what this whole primitive exists to stop. Attack the
   subsequence argument specifically: is "decomposition only adds ASCII base
   letters" true for every canonical decomposition, including compatibility
   cases, Hangul, and precomposed characters whose base is non-ASCII?
2. **Is it too conservative anywhere that matters?** Over-refusal is not safe
   here — it leaves the exact-absence proof unable to authorize. Both directions
   are load-bearing.
3. Is the case-sensitivity probe read-only, and correct when it cannot decide?
4. Are ALL callers migrated to one definition — the sync and async registration
   probes, the porcelain parser, and the removal-verification path that shares
   it? Any semantic change to removal verification must be deliberate.
5. Does `ExactAbsenceCandidateV1` construction bind BOTH source and `common_dir`
   identity? A replaced common-dir with an unchanged source inode must refuse.
6. Any NEW defect, especially in the deepest-existing-ancestor resolution
   (symlinks, permission errors, races between resolution and comparison).
7. Do the tests discriminate, or pass for the wrong reason?
8. Tag every finding WRONG or SMELL; WRONG needs a concrete input/state and the
   incorrect result. WRONG first.
9. End with `VERDICT: APPROVE` or `VERDICT: REJECT` and a one-line SUMMARY.

## Files

- `crates/bridge-core/src/fs_custody.rs` — the primitive and its tests.
- `crates/bridge-worktree/src/host_git.rs` — probes and porcelain parsing.
- `crates/bridge-worktree/src/sweep.rs` — candidate construction, the sweep.

## Spec Refs

- `docs/superpowers/plans/2026-08-17-r2f1b-path-identity-primitive-task.md` —
  the contract and the five instances it closes.
- `docs/superpowers/plans/2026-08-16-r2f1b-3d-t2-root-identity-subslice.md` —
  the class tabulated across both slices.
