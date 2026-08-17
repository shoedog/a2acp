---
task-type: implement
---

# R2f1b — one path-identity primitive for the worktree lane

## Description

Build the shared path-identity primitive this lane has now got wrong **five
times**, and migrate the callers that currently fail open onto it.

Base: `main` (verify `local main == origin/main` before starting — a stale
`--base-ref main` has produced an empty dispatch on this lane before).

### Why this is its own slice

Five instances across two slices, every one the same shape — a path compared by
**spelling** where **identity** was required — and three of them fail OPEN in
proofs whose contract is fail-closed:

| # | Where | Failure |
|---|-------|---------|
| 1 | T2 control root (`backend.rs:2194`) | raw configured spelling pinned, vs canonicalized bound validation |
| 2 | T3a registration probe | byte-exact compare against git's canonical output ⇒ registration read as absent ⇒ **authorize** |
| 3 | T3a missing-tail comparator | absent suffix appended and compared bytewise ⇒ case-insensitive aliases read as different ⇒ **authorize** |
| 4 | T3a candidate source | unchecked string reaches `git -C` ⇒ a relative source queries the wrong repository ⇒ **authorize** |
| 5 | T3a `from_claim` | `common_dir` accepted as `_common_dir` and dropped ⇒ replacing only the common-dir object still passes source revalidation ⇒ **authorize** |

Four repair rounds have patched instances of it in place; a fifth appeared
anyway. The remaining question — *when are two possibly-nonexistent paths
provably different?* — is a design question, which is why it gets a primitive
with its own tests rather than a sixth patch.

Instances 2–5 were invisible to the containerized lane, because Linux has
neither the macOS `/var`→`/private/var` indirection nor a case-insensitive
filesystem. Do not treat a green container run as evidence here.

## What to build

**A tri-state comparison, because two-state cannot express the truth.**

```
Same        — proven to denote the same filesystem object or path
Different   — proven to denote different ones
CannotProve — cannot be established; the caller must refuse
```

Only a **proven** `Different` may let a caller skip or dismiss something.
`CannotProve` always refuses. This asymmetry is the whole point: the previous
implementations collapsed "not provably equal" into "different", which is what
made them fail open.

**The algorithm must handle non-existent paths, because that is the normal case
here** — the exact-absence proof asks about a target that is *supposed* to be
gone. Suggested shape; deviate if you can justify it in the handoff:

1. Both paths exist ⇒ compare object identity (device + inode, the existing
   `verify_payload_directory_identity` machinery) ⇒ `Same` / `Different`.
2. Otherwise resolve each path's **deepest existing ancestor**.
   - Different ancestor objects ⇒ `Different`. The remaining components do not
     exist, so no symlink can rejoin them.
   - Same ancestor object ⇒ compare the remaining components under the
     **filesystem's own name semantics** (below).
3. Name semantics on the shared ancestor:
   - case-sensitive ⇒ any byte difference is `Different`.
   - case-insensitive ⇒ components equal under case folding are `CannotProve`
     (they may denote the same absent entry); components that differ by more
     than case are `Different`.
   - Unicode normalization aliases get the same treatment as case.
4. Anything unresolvable — a permission error, an unreadable ancestor, an
   undeterminable case sensitivity — is `CannotProve`, never `Different`.

**Determining case sensitivity must be read-only.** Do not create files to probe
it. Deriving it from an existing entry under the shared ancestor is acceptable;
so is a platform/filesystem query. If it cannot be determined, that is
`CannotProve` — say which method you chose and why in the handoff.

**Critically — the primitive must not over-refuse.** T3a's repair 3 refused
*every* missing-tail comparison, which is fail-closed but functionally inert: it
broke the pre-existing
`porcelain_registration_check_is_exact_and_handles_locked_records` and would
make the exact-absence proof unable to authorize whenever the repo holds any
other registration. Clearly distinct names under a shared ancestor —
`/managed/wt` vs `/managed/other` — **are** provably different and must classify
`Different`. A primitive that refuses everything is as useless as one that
authorizes everything; the tests below pin both directions.

## Where it lives

`crates/bridge-core/src/fs_custody.rs` is the natural home — the object-identity
machinery is already there and both `bridge-worktree` callers can reach it. If
you place it elsewhere, justify it.

**`bridge-core` compiles for Windows in CI** (via `bridge-store`), and
`liveness` / `namespace_transaction` are `#[cfg(unix)]` while `fs_custody` is
not. This lane has lost five landing rounds to exactly that boundary. Run
`tools/check-nonunix.sh` — it takes ~3 s — and keep the primitive either
portable or correctly gated.

## Callers to migrate

1. `registration_absent_from_porcelain` / `registration_absent_sync` /
   `registration_absent` (`crates/bridge-worktree/src/host_git.rs`) — one shared
   definition for both the sync and async probes, and for the removal-verification
   path that also uses the parser. Changing removal-verification semantics is in
   scope; changing them *silently* is not — call it out.
2. `ExactAbsenceCandidateV1` construction (`crates/bridge-worktree/src/sweep.rs`)
   — fallible; bind **both** the source and the `common_dir` identity from the
   claim. Instance 5 is `common_dir` being accepted and dropped.
3. The T3a comparator, replaced by the primitive.

**Out of scope:** T2's control-root binding (instance 1). It is latent — V3 is
unarmed — and it migrates as part of the V3-arming prerequisite. Note in the
handoff how it *would* migrate, but do not change it.

## Red-first battery

Both directions matter. Each must fail on the base commit:

- Two existing paths, same object via different spellings (a symlinked parent,
  and on macOS `/var` vs `/private/var`) ⇒ `Same`.
- Two existing paths, genuinely different objects ⇒ `Different`.
- Absent sibling names under a shared existing ancestor, differing by more than
  case (`/x/wt` vs `/x/other`) ⇒ `Different`. **This is the anti-over-refusal
  test.**
- Absent names differing only by case, on a case-insensitive filesystem ⇒
  `CannotProve`.
- Absent names under *different* existing ancestors ⇒ `Different`.
- An unreadable or unresolvable ancestor ⇒ `CannotProve`.
- Caller-level: a registration whose recorded spelling differs from git's
  canonical spelling is still detected PRESENT.
- Caller-level: `common_dir` replaced while the source inode is unchanged ⇒
  refuse (instance 5).
- The pre-existing
  `porcelain_registration_check_is_exact_and_handles_locked_records` passes
  unchanged.

## On evidence

Your container has **no compile loop** (implement-lane egress is model APIs
only, ADR-0013), and its `verify: PASS` has twice failed to hold on the host for
this very subsystem. Do not present compile errors as red-first evidence, and do
not claim a test passes because verify was green. State honestly, per test,
whether you ran it. The operator runs the discriminating controls on the host —
including on a case-insensitive filesystem, which your container does not have.

**Falsification license.** The five instances above are operator-verified with
source citations, but the suggested algorithm is a proposal. If you find a
better decomposition, or a case where step 2 is unsound, say so and justify the
deviation rather than forcing this shape.

## Acceptance Criteria

1. A tri-state path-identity primitive exists with the semantics above; only a
   proven `Different` permits a caller to skip.
2. Every red-first test above exists and passes, including **both** the
   anti-over-refusal case and the case-insensitive ambiguity case.
3. `porcelain_registration_check_is_exact_and_handles_locked_records` passes
   unchanged — the primitive does not over-refuse.
4. `host_git`'s sync and async registration checks share ONE definition, and the
   removal-verification path's semantics change is called out explicitly.
5. `ExactAbsenceCandidateV1` construction is fallible and binds both source and
   `common_dir`; a replaced common-dir refuses even when the source inode is
   unchanged.
6. No production behavior outside the named callers changes.
7. `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
   -- -D warnings` clean; workspace suite green; `tools/check-nonunix.sh` passes.
8. `git diff --numstat <base>..HEAD` at most **700** changed lines, reported and
   reconciled in the handoff.
9. The handoff states, per test, whether it was executed, and names the
   case-sensitivity detection method chosen.

## Files

- `crates/bridge-core/src/fs_custody.rs` — proposed home; existing identity machinery.
- `crates/bridge-worktree/src/host_git.rs` — the registration probes and parser.
- `crates/bridge-worktree/src/sweep.rs` — candidate construction and the comparator being replaced.

## Spec Refs

- `docs/superpowers/plans/2026-08-16-r2f1b-3d-t2-root-identity-subslice.md` —
  the design brief this task implements, with all five instances tabulated.
- `docs/superpowers/reviews/2026-08-17-r2f1b-3d-t3a-sol-closure.md` — the counted
  review that specified the tri-state and its trigger analysis.
- `docs/superpowers/plans/2026-08-15-r2f1b-3d-dispatch-brief-DRAFT.md` — the T3a
  park record, including why repair 3 over-refused.

## Commit Message

feat(fs-custody): one tri-state path-identity primitive for the worktree lane

Paths were compared by spelling where identity was required, five times across
two slices, three of them failing open in fail-closed proofs. The primitive
answers Same, Different, or CannotProve, and only a proven difference lets a
caller skip a registration; ambiguity refuses.

Non-existent paths are the normal case here, since the exact-absence proof asks
about a target that should be gone. Those resolve through the deepest existing
ancestor: different ancestor objects prove difference, a shared ancestor defers
to the filesystem's own name semantics, and anything unresolvable refuses.

Migrates the registration probes, the porcelain parser, and exact-absence
candidate construction — which now binds the common-dir identity it previously
accepted and dropped.
