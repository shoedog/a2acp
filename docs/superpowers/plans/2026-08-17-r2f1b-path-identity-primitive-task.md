---
task-type: implement
---

# R2f1b — one path-identity primitive for the worktree lane

## Description

Build the shared path-identity primitive this lane has now got wrong **five
times**, and migrate the callers that currently fail open onto it.

Base: `main` (verify `local main == origin/main` before starting — a stale
`--base-ref main` has produced an empty dispatch on this lane before).

## AMENDMENT 1 — 2026-08-18 — the comparison rule is now PINNED

**Read this before anything below it. Where it conflicts with the original text,
this amendment wins.**

The first two attempts at this slice died the same death, and the cause was
**this spec**, not the implementations. The original text below simultaneously

- required `Different` for absent siblings "differing by more than case" with
  **no ASCII qualifier**, and stated "Unicode normalization aliases get the same
  treatment as case" — requirements whose proof needs Unicode tables; and
- forbade the lane any Unicode dependency.

Those two constraints cannot both be satisfied. Three rules were invented to try
(blanket refusal → case-branch byte compare → ASCII-skeleton subsequence); each
was rejected, and the third was **refuted at closure**: `"\u{00e1}b\u{0307}"`
and `"a\u{0301}\u{1e03}"` both canonically decompose to `a U+0301 b U+0307`,
but their ASCII skeletons are `['b']` and `['a']` — disjoint — so the code
returned `Different` for two canonical-equivalent names. That is a fail-open
proof in a fail-closed contract.

The reviewers enforced the contract correctly. **The contract was the defect.**
So the rule is no longer something the implementer derives — it is fixed below,
normatively, and the refusal rows are pinned as *correct behavior*, not as
over-refusal to be argued away.

### The pinned comparison rule (NORMATIVE — implement exactly this)

Let `A` and `B` be the two paths.

| # | Condition | Verdict |
|---|---|---|
| **A1** | Both `A` and `B` exist | Object identity (device + inode) ⇒ `Same` or `Different` |
| **A2** | Deepest existing ancestors are **different objects** | `Different` |
| **A3** | Same ancestor object; missing tails differ in **component count** | `Different` |
| **A4** | Same ancestor object; missing tails are **byte-equal** | `Same` |
| **A5** | Same ancestor; **any** differing component pair is pure-ASCII on both sides and **not** equal under ASCII case folding | `Different` — **and the case-sensitivity probe is NOT consulted** |
| **A6** | Same ancestor; no pair qualifies under A5, and **any** differing component pair has a non-ASCII byte on either side | `CannotProve` — **in both case branches, unconditionally** |
| **A7** | Same ancestor; every differing pair is pure-ASCII and ASCII-casefold-equal | Case-sensitive ⇒ `Different`; case-insensitive ⇒ `CannotProve`; **mode undeterminable ⇒ `CannotProve`** |
| **A8** | Anything unresolvable — permission error, unreadable ancestor, ambiguous probe | `CannotProve`, never `Different` |

Evaluate A5 **before** A6: a pure-ASCII pair that differs under both case modes
proves the paths differ no matter what any other component contains.

**A6 is the whole point of the amendment.** "Unconditionally" means it applies on
case-**sensitive** ancestors too. The rejected artifact refused non-ASCII only in
the case-insensitive branch and let raw bytes decide in the case-sensitive one —
which is fail-open on case-sensitive-but-normalizing volumes (HFSX; case-sensitive
APFS is still normalization-insensitive). Case sensitivity does not imply
normalization sensitivity. There is a test in the rejected artifact that asserts
the wrong behavior here; it must flip.

**Why A5/A6 need no Unicode tables.** Canonical decomposition of an ASCII
character is the identity, so two distinct pure-ASCII strings are never canonical
equivalents; and Unicode simple case folding restricted to ASCII inputs is ASCII
case folding. That is the entire proof obligation. Do **not** derive any further
Unicode fact — in particular, do not special-case the three characters that are
canonically ASCII-equivalent (U+037E, U+1FEF, U+212A). That refinement is sound
and is deliberately **rejected**: it is another hand-derived table-versioned
theorem of exactly the species that has now died three times.

**A6's functional cost is bounded, and smaller than it looks.** It does not make
the lane refuse whenever a non-ASCII registration exists. When the other
registration's directory **exists** and the target is absent, A2 or A3 proves
`Different` with no name reasoning at all — an existing entry and an ENOENT
lookup can never alias. And `remove_and_verify` runs `git worktree prune` first,
which clears stale registrations unless they are locked. The residual refusal
surface is exactly: **a locked, stale, absent, non-ASCII registration sharing the
deepest existing ancestor with the target.** That is acceptable, it is visible in
logs, and it is the declared trigger for the escalation below.

**Distinguish ENOENT from EACCES.** A2's soundness rests on the absent side being
genuinely absent. A lookup that fails for permission reasons is A8, not absence.

### Excluded by assumption (state it in the code, do not try to handle it)

Filesystems that alias two pure-ASCII names differing by more than case (vfat 8.3
short names and kin) break A5. No string rule survives them. Document the
assumption "no ASCII-aliasing filesystem under the managed root" at the
primitive; do not add machinery for it.

### Severity rule for this lane — asymmetric fix authority

The generator of this whole failure class was review pressure toward `Different`.
Both directions can be WRONG, but they are **not** symmetric and must not be
repaired symmetrically:

- A wrong `Different` is **fail-open**: it authorizes a caller to skip or remove.
- A wrong `CannotProve` is **fail-closed**: it refuses.

Where the contract pins the output — an acceptance criterion, or a pre-existing
passing test — an over-refusal *is* a WRONG with a named input, and is reported
as one. But: **a fail-closed WRONG may never be repaired by widening `Different`
without an explicit soundness argument for the widening.** If no sound widening
exists, the correct repair is to amend the requirement, not to invent a rule.

### Escalation, pre-authorized

If a closure round shows the A1–A8 rule itself unsound, or an operator reports
the A6 refusal surface blocking a real cleanup, escalate to normalize-then-compare
rather than inventing a fourth rule. Note for that day: `icu_normalizer` 2.2.0 is
**already in `Cargo.lock`** and already compiled into the production binary
(reqwest → url → idna → idna_adapter), so it is one direct-dependency edge, not a
new tree. Casefold is **not** — `icu_casemap` would be a genuine addition, and it
is required, because normalization alone does not help the case-insensitive
branch where the entire practical surface lives.

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
3. Name semantics on the shared ancestor: **see the pinned rule table in
   Amendment 1 (rows A3–A7). It is normative and replaces what used to be
   described here.** In particular there is no longer any rule of the form
   "differs by more than case ⇒ `Different`" without an ASCII qualifier, and the
   sentence that used to read "Unicode normalization aliases get the same
   treatment as case" is **withdrawn** — it demanded table knowledge the lane is
   denied, and it is what generated three unsound rules.
4. Anything unresolvable — a permission error, an unreadable ancestor, an
   undeterminable case sensitivity — is `CannotProve`, never `Different`
   (row A8).

**Determining case sensitivity must be read-only.** Do not create files to probe
it. Deriving it from an existing entry under the shared ancestor is acceptable;
so is a platform/filesystem query. If it cannot be determined, that is
`CannotProve` — say which method you chose and why in the handoff.

**The primitive must not over-refuse — as bounded by the pinned rule, and not
one step further.** T3a's repair 3 refused *every* missing-tail comparison, which
is fail-closed but functionally inert: it broke the pre-existing
`porcelain_registration_check_is_exact_and_handles_locked_records` and would make
the exact-absence proof unable to authorize whenever the repo holds any other
registration. **Pure-ASCII** distinct names under a shared ancestor —
`/managed/wt` vs `/managed/other`, and `/managed/123/wt` vs `/managed/123/other`
— **are** provably different and must classify `Different` (row A5), without
consulting the case probe.

But the converse pressure is now closed off: **row A6's refusals are CORRECT
BEHAVIOR, not over-refusal.** A reviewer or repair that reports a non-ASCII
`CannotProve` as an over-refusal defect is applying the withdrawn contract. Cite
this paragraph and Amendment 1's severity rule and reject the finding.

## Where it lives

`crates/bridge-core/src/fs_custody.rs` is the natural home — the object-identity
machinery is already there and both `bridge-worktree` callers can reach it. If
you place it elsewhere, justify it.

**`bridge-core` compiles for Windows in CI** (via `bridge-store`), and
`liveness` / `namespace_transaction` are `#[cfg(unix)]` while `fs_custody` is
not. This lane has lost five landing rounds to exactly that boundary, and
**there is no local gate for it** — one was attempted and withdrawn
(`plans/2026-08-17-nonunix-gate-hermetic-task.md`). So you must reason about it
rather than test it:

- Do not reference `crate::liveness` or `crate::namespace_transaction` from
  `fs_custody` without a `#[cfg(unix)]` guard on the referencing item.
- Anything that becomes unused on non-unix needs
  `#[cfg_attr(not(unix), allow(dead_code))]` — the lane is warning-clean under
  `-D warnings`, so a non-unix `dead_code` fails it just as hard as `E0433`.
- The established fix shape is commit `790b4191`. Follow it.
- State in your handoff which items you gated and why.

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
- Absent **pure-ASCII** sibling names under a shared existing ancestor,
  differing by more than ASCII case (`/x/wt` vs `/x/other`) ⇒ `Different`.
  **This is the anti-over-refusal test** (row A5).
- The same, under an ancestor whose own name is numeric and which holds **no**
  entry from which case sensitivity could be sampled (`/x/123/wt` vs
  `/x/123/other`) ⇒ `Different`, **with the case probe never consulted**. The
  rejected artifact returned `CannotProve` here.
- Absent pure-ASCII names differing only by ASCII case, on a case-insensitive
  filesystem ⇒ `CannotProve` (row A7).
- The same pair on a case-**sensitive** filesystem ⇒ `Different` (row A7).
- Absent names where a differing component carries a non-ASCII byte, on a
  case-**insensitive** ancestor ⇒ `CannotProve` (row A6).
- The same pair on a case-**sensitive** ancestor ⇒ `CannotProve` (row A6,
  unconditional). **This is the row the rejected artifact got wrong**, and it is
  pinned as correct — not as over-refusal.
- The canonical-equivalence counterexample specifically: `"\u{00e1}b\u{0307}"`
  vs `"a\u{0301}\u{1e03}"` as absent siblings ⇒ `CannotProve` under **both**
  case modes. The rejected artifact returned `Different`.
- Missing tails of different component counts (`/x/a` vs `/x/a/b`) ⇒ `Different`
  (row A3), with no probe.
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

**Falsification license — narrowed by Amendment 1.** The five instances above
are operator-verified with source citations, and the *decomposition* (steps 1–2:
existence, object identity, deepest existing ancestor) remains a proposal you may
improve with justification. **The A1–A8 verdict table is not.** It is the settled
answer to the design question that killed three previous attempts; re-deriving it
is the failure mode, not the work. If you find a row unsound, report it and stop
— do not silently implement a different rule.

## Acceptance Criteria

1. A tri-state path-identity primitive exists with the semantics above; only a
   proven `Different` permits a caller to skip.
2. Every red-first test above exists and passes, including **all three**
   directions: the anti-over-refusal `Different` rows, the case-insensitive
   ambiguity row, and the non-ASCII `CannotProve` rows **in both case branches**.
2b. The implementation matches the pinned A1–A8 table in Amendment 1 row for row.
   Any deviation is a spec violation, not a design choice — the falsification
   license below does **not** extend to the pinned rule. If you believe a row is
   wrong, stop and say so; do not implement a different rule.
3. `porcelain_registration_check_is_exact_and_handles_locked_records` passes
   unchanged — the primitive does not over-refuse.
4. `host_git`'s sync and async registration checks share ONE definition, and the
   removal-verification path's semantics change is called out explicitly.
5. `ExactAbsenceCandidateV1` construction is fallible and binds both source and
   `common_dir`; a replaced common-dir refuses even when the source inode is
   unchanged.
6. No production behavior outside the named callers changes.
7. `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
   -- -D warnings` clean; workspace suite green. The non-unix lane has no local
   gate — see the cfg guidance above and state what you gated.
8. `git diff --numstat <base>..HEAD` at most **700** changed lines, reported and
   reconciled in the handoff. **A breach requires an explicit pre-closure waiver
   from the operator** — the previous two rounds breached this cap silently
   (T3a 1,106 vs 750; the primitive 852 vs 700) and a silently-breached cap is
   how a review loop stops converging. Report the numstat honestly; do not
   restate the cap as the measurement.
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
