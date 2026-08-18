---
task-type: implement
---

# Path-identity primitive — repair 2: implement the PINNED rule, and stop losing identity

## Description

Targeted repair on a reviewed artifact. Base: `be7c6708` on branch
`salvage/r2f1b-path-identity-rebased`, which is based on current `main`
(`227c8ecc`).

**This artifact is NOT frozen-unverified.** Unlike repair 1's base, this one
compiles and its suite is green on the host (`fmt` clean, `clippy` clean,
4,147 passed / 0 failed / 13 ignored). A counted closure then read the complete
852-line range and returned **REJECT with six correctness blockers and three
smells**. So: the plumbing works, the *decisions* are wrong. Do not rebuild it.
Fix the named defects in place.

**The central design question is no longer yours to answer.** The primitive spec
now carries **Amendment 1**, a normative A1–A8 verdict table. Three rules were
invented for this slice before it and all three were rejected; the third was
refuted at closure. The amendment exists so that a fourth is not attempted.
Implement the table exactly. If you believe a row is unsound, **stop and say so
in your handoff** — do not implement a different rule.

**This task carries the authoritative copy of the rule.** The primitive spec that
holds Amendment 1 lives on a planning branch and is **NOT present in your
checkout** — do not go looking for it, and do not treat its absence as a missing
input. Everything normative is reproduced below in full.

### The pinned rule — NORMATIVE, implement exactly this

| # | Condition | Verdict |
|---|---|---|
| A1 | Both paths exist | device+inode identity ⇒ `Same` / `Different` |
| A2 | Deepest existing ancestors are different objects | `Different` |
| A3 | Same ancestor; missing tails differ in component count | `Different` |
| A4 | Same ancestor; missing tails byte-equal | `Same` |
| A5 | Same ancestor; **any** differing pair is pure-ASCII both sides and **not** ASCII-casefold-equal | `Different`, **probe NOT consulted** |
| A6 | Same ancestor; no A5 pair, and **any** differing pair has a non-ASCII byte either side | `CannotProve`, **both case branches, unconditionally** |
| A7 | Same ancestor; every differing pair pure-ASCII and ASCII-casefold-equal | sensitive ⇒ `Different`; insensitive ⇒ `CannotProve`; **undeterminable ⇒ `CannotProve`** |
| A8 | Unresolvable — permission error, unreadable ancestor, ambiguous probe | `CannotProve`, never `Different` |

**Evaluation order.** A5 is evaluated **before** A6: a pure-ASCII pair that
differs under both case modes proves the paths differ no matter what any other
component contains. `CannotProve` refuses; only a proven `Different` lets a
caller skip or remove.

**A6 is unconditional — this is the whole point.** "Both case branches" means it
applies on case-**sensitive** ancestors too. The rejected artifact refused
non-ASCII only in the case-insensitive branch and let raw bytes decide in the
case-sensitive one. Case sensitivity does **not** imply normalization
sensitivity: HFSX and case-sensitive APFS are case-sensitive *and*
normalization-insensitive, so a byte comparison there is fail-open.

**Why A5 and A6 need no Unicode tables.** Canonical decomposition of an ASCII
character is the identity, so two distinct pure-ASCII strings are never canonical
equivalents; and Unicode simple case folding restricted to ASCII inputs is ASCII
case folding. That is the entire proof obligation — implement nothing more.

**Do not derive further Unicode facts.** In particular, do not special-case the
three characters that are canonically ASCII-equivalent (U+037E, U+1FEF, U+212A).
That refinement is sound and is deliberately **rejected**: it is another
hand-derived, table-versioned theorem of exactly the species that has now died
three times in this slice.

**Distinguish ENOENT from EACCES.** A2's soundness rests on the absent side being
genuinely absent. A lookup that fails for permission reasons is row A8, not
absence.

**Excluded by assumption.** Filesystems that alias two pure-ASCII names differing
by more than case (vfat 8.3 short names and kin) break A5. No string rule
survives them. State the assumption "no ASCII-aliasing filesystem under the
managed root" in a comment at the primitive; add no machinery for it.

**A6's functional cost is bounded — do not "fix" it.** A6 does not make the lane
refuse whenever a non-ASCII registration exists. When the other registration's
directory **exists** and the target is absent, row A2 or A3 proves `Different`
with no name reasoning at all — an existing entry and an ENOENT lookup can never
alias. And `remove_and_verify` runs `git worktree prune` first, clearing stale
registrations unless they are locked. The entire residual refusal surface is: **a
locked, stale, absent, non-ASCII registration sharing the deepest existing
ancestor with the target.** That is accepted, and it is the declared trigger for
escalation — not a defect to engineer around.

### Severity rule for this lane — asymmetric fix authority

Both directions can be WRONG, but they are **not** symmetric:

- A wrong `Different` is **fail-open**: it authorizes a caller to skip or remove.
- A wrong `CannotProve` is **fail-closed**: it refuses.

**A fail-closed WRONG may never be repaired by widening `Different` without an
explicit soundness argument for the widening.** Review pressure toward
`Different` is precisely what generated the three dead rules. If you find
yourself reasoning "this refusal is too broad, so it should return `Different`" —
stop; that is the failure mode, and A6's refusals are pinned as correct behavior.

## The six blockers

### B1 — the Unicode `Different` proof is false, in BOTH branches

`ascii_skeletons_could_normalize_alike` (`fs_custody.rs`, ~line 1618) claims
canonical decomposition only ever ADDS ASCII base letters, so one skeleton must
be a subsequence of the other. **That is false.** `"\u{00e1}b\u{0307}"` and
`"a\u{0301}\u{1e03}"` both canonically decompose to `a U+0301 b U+0307` — they
are the same name — but their skeletons are `['b']` and `['a']`, disjoint, so the
function returns `false` and the comparator returns `Different` for two
equivalent spellings. That is a fail-open verdict in a fail-closed contract.

**Delete the function and its doc comment.** Do not repair the theorem.

The second mechanism is separate and must also be fixed: in
`compare_missing_tail` (~line 1599) the `if case_sensitive { return Different }`
arm lets raw bytes decide **before** any non-ASCII check. Case sensitivity does
not imply normalization sensitivity — HFSX and case-sensitive APFS are
case-sensitive *and* normalization-insensitive — so this is fail-open on those
volumes. Row A6 is unconditional: it applies on case-sensitive ancestors too.

There is a test that **pins the wrong behavior as an assertion** (around
`fs_custody.rs:2956-2961`: a non-ASCII pair asserted `Different` under a
case-sensitive ancestor). **It must flip to `CannotProve`.** Flipping it is the
point of the repair, not a regression — say so explicitly in your handoff.

### B2 — sequential ancestor resolutions can return `Different` for one path

`compare_path_identities` (~line 1653) resolves left and right through two
separate `deepest_existing_path` calls and treats differing ancestor identities
as proof (A2). For the **identical string** `/root/wt` passed as both arguments,
a rename of `/root` between the two resolutions yields two different ancestor
identities and the function returns `Different` — for a path compared against
itself. The only justified answer under drift is `CannotProve`.

Fix, both parts:
- **Spelling short-circuit:** if the two inputs are byte-identical as paths,
  return `Same` without resolving anything. This alone fixes the self-compare
  and is what `registration_absent_from_porcelain`'s guard actually needs.
- **Stability check:** for genuinely different spellings, after computing a
  verdict that depends on ancestor identity, re-resolve and confirm both ancestor
  identities are unchanged. Any drift ⇒ `CannotProve`. Do not return `Different`
  from a pair of observations you have not shown to be contemporaneous.

`case_sensitive_at` is likewise reached through an unpinned canonical string
after the fact; fold it into the same stability window.

### B3 — case sensitivity is probed in the wrong directory

`case_sensitive_at` (~line 1566) first calls
`probe_case_sensitivity(parent, name, ...)` — it changes the **ancestor's own
basename** and looks it up **in the ancestor's parent**. That measures the
parent's entry semantics, not the semantics that govern children *inside* the
ancestor. Linux supports per-directory casefold (ext4/F2FS `+F`): a casefold
worktree root under a case-sensitive parent makes this probe answer
"case-sensitive", after which `wt` and `WT` classify `Different` although they
alias. Fail-open.

Fix: **delete the parent probe.** Sample only entries *inside* the shared
ancestor — the `read_dir` fallback already present is the correct scope. If no
entry can be sampled, the answer is `None` ⇒ A7's undeterminable ⇒ `CannotProve`
(and after B6, most cases no longer need the probe at all).

### B4 — source / `common_dir` identity is not held across the git call

`sweep.rs:98` revalidates source and common-dir, then `host_git.rs:163` runs the
target probe and `git worktree list` with **no** identity check afterwards. After
`revalidate_source()` succeeds, replacing only `source/.git` with another valid
common directory — leaving the source inode untouched — makes git query the
replacement repository, see no registration, and return `BothAbsent`. Today that
is an effect-free sweep decision; T3b consumes it and it becomes destructive.

Fix: revalidate **both** the source and the `common_dir` identity **after** the
git subprocess returns, before the observation is used. Any drift ⇒ refuse
(error / unproven), never `BothAbsent`. Strict ABA protection needs descriptor
binding; if you cannot bind descriptors here, implement post-command
revalidation and **state the residual window in your handoff** — do not claim
ABA safety you did not implement.

### B5 — `CannotProve` is collapsed into a definite "registered"

`registration_absent_from_porcelain` (`host_git.rs:131`) returns `Ok(false)` for
**both** `Same` and `CannotProve` (`!= Different` ⇒ present). At
`host_git.rs:224` `Ok(false)` becomes `RecoveryLocatorV1::RegisteredWorktree {}`
— a definite claim — and only `Err` becomes `RegistrationUnproven`. The tri-state
the primitive exists to carry is destroyed at its first consumer, and the false
locator is durably published (`backend.rs:4287`).

Constructible: target `resume` is unregistered while an unrelated stale
`résumé` registration makes the comparator answer `CannotProve` (row A6); the
record then asserts `resume` IS registered.

Fix: propagate three states, not two. Match `Same` / `Different` / `CannotProve`
explicitly at every hop — `registration_absent_from_porcelain`,
`registration_absent_from_output`, `registration_absent_sync`,
`registration_absent`, `observe_exact_absence`, `classify_custody_add_failure`.
`CannotProve` ⇒ `RegistrationUnproven` / the refusing path. Changing these
signatures off `bool` is in scope and expected.

### B6 — mode-independent differences are refused before comparison

`compare_path_identities` (~line 1670) requires `case_sensitive_at` to answer
`Some` **before** it will call `compare_missing_tail` at all. With an existing
ancestor named `123` holding no entries, `alternate_ascii_case` finds no
alphabetic byte and `read_dir` yields nothing ⇒ `None` ⇒ `CannotProve`. So
`/x/123/wt` vs `/x/123/other` refuses, although those names differ under **either**
case mode. Different tail lengths refuse the same way, for the same reason.

Fix: **evaluate the verdict before probing.** Apply A3, then A5, then A6. Only
reach for the case probe when the answer genuinely depends on the mode — row A7,
all differing pairs pure-ASCII and ASCII-casefold-equal. This is the both-modes-
first shape the closure prescribed, and under Amendment 1 it also shrinks B3's
remaining surface to the single probe-dependent row.

## B7 — a seventh instance, found after the closure

`probe_case_sensitivity` reads `Err(NotFound) => Some(true)`. The entry it probes
was sampled from a `read_dir` snapshot. If that entry is **deleted** between the
snapshot and the alternate-case lookup, the lookup returns `NotFound` on a
case-**insensitive** directory and the probe reports "case-sensitive" — after
which an ASCII case-only pair classifies `Different`. Fail-open, same
unpinned-identity family as B2/B4.

Fix: on `NotFound` for the alternate spelling, re-`symlink_metadata` the
**original** entry. If it is now absent, or its identity no longer matches
`expected`, the sample is void ⇒ return `None` and try the next entry. Only a
still-present original licenses the `Some(true)` conclusion.

## Also in scope

- **SMELL 1 — the load-bearing behavior has no discriminating tests.** The
  existing tests inject the case-mode boolean directly, so nothing proves the
  read-only probe detects a case-insensitive ancestor. Add: a probe test that
  exercises `case_sensitive_at` itself; a caller test for the incident shape (an
  **absent** registered target, not the existing-symlink alias currently
  covered); a dangling-final-symlink regression for the `try_exists` migration;
  and a common-dir swap that lands **during** the git observation rather than
  before revalidation.
- **SMELL 3 — the committed handoff on this artifact is materially false.**
  `docs/superpowers/reviews/2026-08-17-r2f1b-3d-t3a-path-identity-handoff.md`
  claims NFC normalization the code never performed, says tests were not executed
  when host evidence was later supplied, and reports 700 changed lines against an
  actual 852. Rewrite it to match what this repair actually does, and reconcile
  the numstat mechanically rather than restating the cap.
- **ASCII leaf premise.** Managed worktree leaves are `{owner}-{run}-{hash}`
  (`provider_path.rs:49-53`); the hash is hex but `owner` and `run` are
  unvalidated operator config strings. "Bridge leaves are ASCII" is therefore
  convention, not construction. Either validate `worktrees.owner` / `run` as
  ASCII at config load, or document the assumption where the primitive states it.
  Validation is preferred; say which you chose.

**Explicitly out of scope:** T2's control-root binding (it migrates with the
V3-arming prerequisite); the reaper `kill_on_drop` change; any new dependency.

## Red-first battery

Each must fail on `be7c6708` and pass after. Where a listed test already exists
with the **opposite** assertion, flipping it counts — name it as a flip.

- `"\u{00e1}b\u{0307}"` vs `"a\u{0301}\u{1e03}"` as absent siblings ⇒
  `CannotProve` under a case-**insensitive** ancestor. Currently `Different`.
- The same pair under a case-**sensitive** ancestor ⇒ `CannotProve`. Currently
  `Different` (this is the assertion that must flip).
- Any non-ASCII differing pair under a case-sensitive ancestor ⇒ `CannotProve`.
- `/x/wt` vs `/x/other` ⇒ `Different` — the anti-over-refusal row, still required.
- `/x/123/wt` vs `/x/123/other`, ancestor `123` empty and non-alphabetic ⇒
  `Different`, **with the case probe never consulted** (assert the probe is not
  called, or that the result holds when the probe is forced to `None`).
- `/x/a` vs `/x/a/b` (different tail component counts) ⇒ `Different`, no probe.
- Pure-ASCII case-only pair, case-insensitive ancestor ⇒ `CannotProve`;
  case-sensitive ancestor ⇒ `Different`.
- The identical string compared with itself while its ancestor is renamed
  between resolutions ⇒ `Same` (short-circuit) — and a genuinely different pair
  under the same drift ⇒ `CannotProve`, never `Different`. Use a barrier.
- A casefold-enabled ancestor under a case-sensitive parent ⇒ the probe does not
  report "case-sensitive". Inject the probe if the fixture is unavailable, and
  say which you did.
- `probe_case_sensitivity` with the sampled entry deleted before the alternate
  lookup ⇒ `None`, not `Some(true)`.
- `common_dir` replaced **after** `revalidate_source()` and **during** the git
  observation ⇒ refuse, not `BothAbsent`. Barrier-controlled.
- An unregistered target plus an unrelated non-ASCII registration that forces
  `CannotProve` ⇒ the persisted locator is `RegistrationUnproven`, **not**
  `RegisteredWorktree`. Assert the persisted record, not just the parser result.
- `porcelain_registration_check_is_exact_and_handles_locked_records` still passes
  unchanged.

## On evidence

Your container has **no compile loop**, and its `verify: PASS` has twice failed
to hold on the host for this exact subsystem. Linux has neither the macOS
`/var`→`/private/var` indirection nor a case-insensitive filesystem, so the rows
that matter most here are the rows your container cannot run. Do not present a
green verify as evidence a test passes. State **per test** whether you executed
it. The operator runs the discriminating controls on the host.

`bridge-core` compiles for Windows in CI while `liveness` and
`namespace_transaction` are `#[cfg(unix)]`. This lane has lost five landing
rounds to that boundary and there is no local gate for it. Do not reference
those modules from `fs_custody` without a `#[cfg(unix)]` guard on the
referencing item; anything that becomes unused on non-unix needs
`#[cfg_attr(not(unix), allow(dead_code))]`, because the lane is warning-clean
under `-D warnings`. The established shape is commit `790b4191`. State what you
gated.

## Acceptance Criteria

1. B1–B7 are each fixed, and the handoff names the fix and the test per item.
2. `ascii_skeletons_could_normalize_alike` no longer exists.
3. The implementation matches the A1–A8 table in this task row for row. Any
   deviation is a spec violation reported in the handoff, not a design choice.
4. Row A6 refusals are present in **both** case branches, and the test that
   asserted otherwise has been flipped and identified as a flip.
5. The case probe is consulted **only** for row A7; A3/A5/A6 resolve without it.
6. `registration_absent*` and `observe_exact_absence` carry three states, and a
   `CannotProve` reaches the durable record as `RegistrationUnproven`.
7. Every red-first row above exists as a test and passes; each is marked
   executed or not-executed honestly.
8. `porcelain_registration_check_is_exact_and_handles_locked_records` passes
   unchanged.
9. No production behavior outside the named callers changes; no new dependency.
10. `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
    -- -D warnings` clean; `cargo test --workspace --locked --no-fail-fast`
    green, with the totals reported.
11. `git diff --numstat be7c6708..HEAD` at most **500** changed lines for the
    repair delta, reported and reconciled mechanically. A breach requires an
    explicit pre-closure waiver from the operator — the previous two rounds
    breached their caps silently, and a silently-breached cap is how a review
    loop stops converging.

## Files

- `crates/bridge-core/src/fs_custody.rs` — the primitive, the probe, the comparator.
- `crates/bridge-worktree/src/host_git.rs` — the tri-state collapse and the git call site.
- `crates/bridge-worktree/src/sweep.rs` — candidate construction and revalidation.
- `crates/bridge-worktree/src/backend.rs` — the durable locator projection (read to confirm; change only if B5 requires it).
- `docs/superpowers/reviews/2026-08-17-r2f1b-3d-t3a-path-identity-handoff.md` — the stale artifact handoff to rewrite.

## Spec Refs

**These live on a planning branch and are NOT in your checkout.** They are listed
for provenance only. Everything you need is in this task; their absence is not a
missing input and is not a reason to pause.

- `docs/superpowers/plans/2026-08-17-r2f1b-path-identity-primitive-task.md`
  § AMENDMENT 1 — the contract this task reproduces.
- `docs/superpowers/reviews/2026-08-17-path-identity-sol-closure.md` — the counted
  closure that rejected `be7c6708`, with each blocker's constructible state.
- `docs/superpowers/reviews/2026-08-17-decision-analysis-fable.md` — the decision
  record behind Amendment 1, and the source of B7.

The one file in this list you **do** have and must rewrite is
`docs/superpowers/reviews/2026-08-17-r2f1b-3d-t3a-path-identity-handoff.md`
(SMELL 3, above).

## Commit Message

fix(fs-custody): implement the pinned path-identity rule and stop losing identity

The skeleton-subsequence proof was false: two names that canonically decompose
alike can have disjoint ASCII skeletons, so the comparator returned Different for
two spellings of one entry. The function is deleted rather than repaired, and the
comparison now follows the spec's pinned A1-A8 table: non-ASCII differing pairs
refuse in BOTH case branches, pure-ASCII pairs that differ under either mode
prove Different without consulting the case probe, and the probe is reached only
when the answer actually depends on the mode.

Six further identity defects go with it. The case probe measured the ancestor's
parent instead of the ancestor, so a casefold directory under a case-sensitive
parent read as case-sensitive. Two ancestor resolutions were compared without
showing they were contemporaneous, so a rename between them made a path differ
from itself. Source and common-dir identity were revalidated before the git
subprocess but never after it, so swapping only .git left the observation
authoritative. CannotProve was folded into Ok(false) and published as a definite
RegisteredWorktree, destroying the tri-state at its first consumer. A numeric
ancestor with no sampleable entry refused pairs that differ under either case
mode. And the case probe read a deleted sample's ENOENT as evidence of case
sensitivity.

Every ambiguous case still refuses. No Unicode table is encoded, and no
dependency is added.
