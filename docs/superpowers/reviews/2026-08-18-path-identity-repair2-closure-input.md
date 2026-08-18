---
task-type: code-review
---

# Path-identity primitive — repair 2 counted closure

## Description

Review `git diff be7c6708..HEAD` in this checkout — the **repair delta**, 1,014
changed lines (820 additions, 194 deletions) across `crates/bridge-core/src/fs_custody.rs` (the primitive),
`crates/bridge-worktree/src/host_git.rs` and `.../backend.rs` and `.../custody.rs`
(the migrated callers), plus the artifact handoff.

The base `be7c6708` was itself read in full by a previous counted closure (852
lines) which returned **REJECT with six correctness blockers and three smells**.
This round is the targeted repair of those findings. Your job is two things, in
this order:

1. **Verify each named blocker is actually closed** — not plausibly addressed,
   closed. Each is listed below with the constructible state that proved it.
2. **Find anything the repair itself introduced.** The delta is large and touches
   a fail-closed proof; a fix that opens a new hole is worse than the hole it
   closed.

You are **not** being asked to re-litigate the design. See "The rule is pinned"
below — that part is settled, and re-opening it is the specific failure this
slice has already suffered three times.

### The rule is PINNED — read this before judging any verdict

This slice previously died three times because the *spec* demanded a proof that
needed Unicode tables while forbidding any Unicode dependency. Three rules were
invented to satisfy both constraints; all three were rejected, and the third was
refuted at closure. **The reviewers enforced the contract correctly each time.
The contract was the defect.** It has been amended. The comparison rule is now
fixed, normative, and not the implementer's to choose:

| # | Condition | Verdict |
|---|---|---|
| A1 | Both paths exist | device+inode identity ⇒ `Same` / `Different` |
| A2 | Deepest existing ancestors are different objects | `Different` |
| A3 | Same ancestor; missing tails differ in component count | `Different` |
| A4 | Same ancestor; missing tails byte-equal | `Same` |
| A5 | Same ancestor; **any** differing pair is pure-ASCII both sides and **not** ASCII-casefold-equal | `Different`, **probe NOT consulted** |
| A6 | Same ancestor; no A5 pair, and **any** differing pair has a non-ASCII byte either side | `CannotProve`, **both case branches, unconditionally** |
| A7 | Same ancestor; every differing pair pure-ASCII and ASCII-casefold-equal | sensitive ⇒ `Different`; insensitive ⇒ `CannotProve`; undeterminable ⇒ `CannotProve` |
| A8 | Unresolvable — permission error, unreadable ancestor, ambiguous probe | `CannotProve`, never `Different` |

A5 is evaluated before A6. The soundness argument is two lines and needs no
tables: canonical decomposition of an ASCII character is the identity, so two
distinct pure-ASCII strings are never canonical equivalents; and Unicode simple
case folding restricted to ASCII inputs is ASCII case folding.

**A6's refusals are CORRECT BEHAVIOUR, pinned as such.** A finding that reports a
non-ASCII `CannotProve` as over-refusal, functional inertness, or a usability
defect is applying the withdrawn contract — do not raise it. A6 applies on
case-**sensitive** ancestors too: case sensitivity does not imply normalization
sensitivity (HFSX and case-sensitive APFS are case-sensitive *and*
normalization-insensitive), so letting bytes decide there is fail-open.

Correspondingly, one test's assertion was **deliberately flipped** from
`Different` to `CannotProve` for a non-ASCII pair under a case-sensitive ancestor.
That flip is the repair, not a regression.

#### Severity rule for this lane — asymmetric fix authority

- A wrong `Different` is **fail-open**: it authorizes a caller to skip or remove.
- A wrong `CannotProve` is **fail-closed**: it refuses.

Both can be WRONG, but **a fail-closed WRONG may never be repaired by widening
`Different` without an explicit soundness argument for the widening.** Pressure
toward `Different` is precisely what generated the three dead rules. If a finding
of yours would be fixed by making the comparator answer `Different` more often,
that finding needs a soundness proof attached or it is not a finding.

Tag every item **WRONG** or **SMELL**. WRONG means the code provably does the
wrong thing — name the input or state and the incorrect result. SMELL is a risk or
gap with no demonstrated incorrect behaviour. A finding without a concrete failure
scenario is a SMELL, never a blocker.

### The six blockers this repair must close

Each was proved against `be7c6708` with the state named. Confirm closure.

1. **The Unicode `Different` proof was false.** `"\u{00e1}b\u{0307}"` and
   `"a\u{0301}\u{1e03}"` canonically decompose alike but have disjoint ASCII
   skeletons, so the comparator returned `Different` for one entry's two
   spellings. Second mechanism: the case-sensitive branch let raw bytes decide
   before any non-ASCII check. Expect `ascii_skeletons_could_normalize_alike` to
   be **gone**, not repaired, and A6 to hold in both branches.
2. **Two ancestor resolutions compared as if contemporaneous.** Renaming the
   ancestor between them made a path differ from *itself*. Expect a
   byte-identical short-circuit plus a stability re-check, with drift ⇒
   `CannotProve`.
3. **The case probe measured the wrong directory** — it altered the ancestor's
   basename and looked it up in the ancestor's **parent**, so an ext4/F2FS
   casefold root under a case-sensitive parent read as case-sensitive. Expect
   sampling only *inside* the shared ancestor.
4. **Source/`common_dir` identity was not held across the Git subprocess.**
   Replacing only `source/.git` after `revalidate_source()` let Git query a
   different repository and return `BothAbsent`. Expect post-command
   revalidation. Note the handoff explicitly does **not** claim ABA safety;
   descriptor binding is out of scope, so judge the disclosure, not the absence.
5. **`CannotProve` was collapsed into a definite "registered".** Both `Same` and
   `CannotProve` returned `Ok(false)`, which became `RegisteredWorktree` and was
   durably published. Expect three states threaded to the record, with
   `CannotProve` ⇒ `RegistrationUnproven`.
6. **Mode-independent differences were refused before comparison.** An existing
   ancestor named `123` with no sampleable entry made the probe return `None`, so
   `/x/123/wt` vs `/x/123/other` refused although they differ under either mode.
   Expect A3/A5/A6 to resolve before any probe.

**B7, found after that closure and fixed here:** `probe_case_sensitivity` read
`Err(NotFound) => Some(true)`, so an entry deleted between the `read_dir` snapshot
and the alternate-case lookup made a case-*insensitive* directory report
case-sensitive — fail-open. Expect the sampled entry to be revalidated.

### What the operator already ran and found

Treat this as supplied evidence, not as your own.

- **Host gate on the artifact:** `cargo fmt --all -- --check` exit 0;
  `cargo clippy --workspace --all-targets --locked -- -D warnings` exit 0 with
  zero warning/error lines; `cargo test --workspace --locked --no-fail-fast`
  exit 0, **4,157 passed / 0 failed / 13 ignored across 91 test binaries**. Run on
  macOS — a case-insensitive filesystem with the `/var`→`/private/var`
  indirection, which is where three of this lane's worst defects lived and which
  the Linux container cannot reproduce. **Non-Unix is reasoning-only; no gate
  exists for it.**
- **The first counted closure of this repair rejected it on a red test**, and the
  operator diagnosed the cause: the B5 end-to-end fixture derived its stale
  non-ASCII sibling with `replacen("run", "rún", 1)`, but the leaf is
  `ownr-r2f1a-<hash>` — no `"run"` substring — so the replace was a no-op,
  `stale == target`, and the fixture registered the *target*, deleted it and
  locked it. `RegisteredWorktree` was the correct answer to the state actually
  built; the intended A6 ambiguity was never constructed. The fixture now appends
  a non-ASCII component and asserts its own precondition. **Scrutinise that fix:
  a fixture that silently tests nothing is exactly what let a red gate look like
  a code defect.**
- **AC11's 500-line cap is waived by explicit operator decision.** Do not raise
  the delta size as a finding; the waiver is on the record and its reason is that
  the largest additions are the discriminating regressions the previous closure
  demanded. The waiver was granted at 927 lines and the delta is now 1,014: the
  additional ~87 lines are **documentation only** — the per-row execution table
  and disclosures this same closure asked for as MINOR findings. No code was added
  after the waiver.

### What to weigh hardest

- **Did any fix open a new hole?** Especially B2's stability re-check and B4's
  post-command revalidation — both add windows and branches to a fail-closed
  proof. A refusal path that now returns `Different` on some schedule is a
  blocker.
- **Do the new tests actually construct the states they name?** The B5 fixture
  defect above proves this lane can ship a test that passes vacuously or fails
  spuriously. Check the barrier tests and the real-subprocess tests especially:
  a barrier that fires after the observation it meant to interleave with proves
  nothing.
- **Is the tri-state genuinely threaded, or only at the top?** Follow
  `CannotProve` from the comparator to the persisted record at every hop.
- **`bridge-core` compiles for Windows in CI** while `liveness` and
  `namespace_transaction` are `#[cfg(unix)]`. This lane lost five landing rounds
  to that boundary and there is no local gate. Check the cfg gating by reasoning;
  an ungated reference or a non-unix `dead_code` fails CI under `-D warnings`.

## Acceptance Criteria

A useful review of this repair must:

1. **Rule the verdict `APPROVE` or `REJECT`**, with every blocker enumerated. Each
   blocker must name the input or state and the incorrect result, so the repair is
   bounded and the loop can converge. If the only surviving items are SMELLs, say
   so and APPROVE — a deferred SMELL with a ledger entry is a valid outcome, and
   manufacturing a blocker to avoid approving is itself a failure mode here.
2. **State, per blocker B1–B6 and B7, whether it is closed** — closed, not
   plausibly addressed. Cite the code that closes it. A blocker you cannot confirm
   closed is a REJECT.
3. **Confirm the implementation matches the pinned A1–A8 table row for row**, and
   report any deviation as a spec violation. Do not propose an alternative rule.
4. **Report anything the repair itself introduced**, especially in B2's stability
   re-check and B4's post-command revalidation — both add branches and windows to
   a fail-closed proof. A path that now yields `Different` on some schedule is a
   blocker; a path that now refuses more is not.
5. **Judge whether the new tests construct the states they name.** For each
   barrier and real-subprocess test, say whether the barrier actually interleaves
   with the observation it claims to bracket, or whether it could fire before or
   after and still pass. The B5 fixture defect described above is precedent: a test
   two reviewers cited as confirming B5 was building a different scenario entirely.
6. **Assess the `#[cfg(unix)]` gating by reasoning**, since no non-unix gate
   exists. Name any item that would fail Windows CI under `-D warnings`, whether by
   an ungated reference or by becoming dead code.
7. **Tag every finding WRONG or SMELL**, and attach a soundness argument to any
   finding whose remedy would widen `Different`.

Do not raise the delta size, and do not raise A6's non-ASCII refusals as
over-refusal. Both are settled above by explicit operator decision.

## Files

- `crates/bridge-core/src/fs_custody.rs` — the primitive, the comparator, the case probe.
- `crates/bridge-worktree/src/host_git.rs` — the tri-state and the Git call site.
- `crates/bridge-worktree/src/backend.rs` — the durable locator projection and the B5 end-to-end test.
- `crates/bridge-worktree/src/custody.rs` — the recovery-locator type.
- `docs/superpowers/reviews/2026-08-17-r2f1b-3d-t3a-path-identity-handoff.md` — the artifact's own handoff, including its per-row execution table and its disclosed limits. Judge whether its claims match the code.

## Spec Refs

- The pinned A1–A8 table and the asymmetric fix-authority rule are reproduced in
  full in the Description above; they are normative for this review. The spec they
  come from lives on a planning branch and is not in this checkout — its absence is
  not a missing input.
