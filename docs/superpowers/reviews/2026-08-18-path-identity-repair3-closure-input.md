---
task-type: code-review
---

# Path-identity primitive — repair 2 counted closure

## Description

Review `git diff 39f8c3e1..HEAD` in this checkout — the **repair-3 delta**, 223
changed lines (171 additions, 52 deletions) across
`crates/bridge-core/src/fs_custody.rs`, `crates/bridge-worktree/src/host_git.rs`,
`crates/bridge-worktree/src/backend.rs`, and the artifact handoff.

The base `39f8c3e1` was read in full by the immediately preceding counted closure,
which ruled **B1 and B3–B7 closed** and returned **REJECT on one blocker plus three
smells**. This round is the targeted repair of exactly those four items, and
nothing else. Your job is two things, in this order:

1. **Verify W1 and S1–S3 are actually closed** — not plausibly addressed, closed.
   Each is stated below with the constructible state or evidence gap that proved
   it. The previously-closed B1 and B3–B7 are **not** re-opened for review unless
   this delta broke one; if it did, that outranks everything else here.
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

### The four items this repair must close

**W1 — BLOCKER in the base: the stability bracket ignored resolved-tail drift.**
`ancestors_are_stable_with_resolver` re-resolved both paths but compared only
ancestor `(dev, ino)`, ignoring the resolved canonical path and missing tail.
Constructible state: `/R` exists, `/R/link` and `/R/foo` absent; comparing
`/R/link/foo` with `/R/foo` gives tails `["link","foo"]` vs `["foo"]` ⇒ A3
`Different`; create `/R/link -> /R` before revalidation and both re-resolve to
ancestor `/R` with tail `["foo"]` while **both identities stay unchanged**, so the
bracket passed and returned the stale `Different` for two paths that now alias.
Fail-open: the porcelain registration spelled the first way is then discarded as
unrelated, yielding `Absent` ⇒ `BothAbsent` ⇒ `Authorized`.

**S1 — the B4 common-dir barrier could pass for the wrong reason.** The swap hook
ran after `spawn`, spawned without piped output, and the test asserted only
`.is_err()`, so an ordinary Git failure satisfied it.

**S2 — the B5 end-to-end fixture could not distinguish A6 from a Git failure.**
Both `Ok(CannotProve)` and any `worktree list` `Err` map to `RegistrationUnproven`,
and the test never asserted the porcelain succeeded or carried the stale path.

**S3 — B7 had no unchanged-sample positive control.** A mutant making
`sampled_entry_still_matches` always return `None` passed both negative tests.

**Weigh the fix direction.** Every change in this delta must narrow `Different` to
`CannotProve` or strengthen a test. If anything here **widens** `Different`, or
weakens an existing assertion, that is a blocker.

### What the operator already ran and found

Treat this as supplied evidence, not as your own.

- **Host gate on this artifact** (`4e054979`): `cargo fmt --all -- --check` exit 0;
  `cargo clippy --workspace --all-targets --locked -- -D warnings` exit 0, zero
  warning/error lines; `cargo test --workspace --locked --no-fail-fast` exit 0 —
  **4,161 passed / 0 failed / 13 ignored across 91 test binaries**. Run on macOS, a
  case-insensitive filesystem with the `/var`→`/private/var` indirection, which the
  Linux container cannot reproduce and where three of this lane's worst defects
  lived. **Non-Unix is reasoning-only; no gate exists for it.**
- **The operator mutation-tested the new tests**, because this artifact has already
  shipped two tests that looked like evidence and were not:
  - Reverting the W1 fix to the identity-only bracket makes
    `path_identity_refuses_missing_tail_drift_with_unchanged_ancestor_identity` and
    `path_identity_refuses_canonical_path_drift_with_unchanged_ancestor_identity`
    **both FAIL**, while `path_identity_preserves_a_stable_resolver_verdict`
    correctly stays green. The drift tests discriminate; the control is correctly
    insensitive.
  - Forcing `sampled_entry_still_matches` to always return `None` makes only
    `case_probe_keeps_an_unchanged_sample` fail — both pre-existing negative tests
    pass. S3 was a real gap and the control closes it.
  - `DeepestExistingPathV1` derives `PartialEq`/`Eq` over exactly `canonical`,
    `identity`, `missing_tail`, with no manual impl and no skipped field, so the
    new full-struct comparison covers all three.
  These are operator findings. **You may disprove them** — if a test does not in
  fact discriminate, say so.
- **The delta is 223 lines against a 250-line cap** — inside it. Size is not a
  finding this round.

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
2. **State, per item W1, S1, S2 and S3, whether it is closed** — closed, not
   plausibly addressed. Cite the code that closes it. An item you cannot confirm
   closed is a REJECT. Do not re-adjudicate B1 or B3–B7 unless this delta broke
   one.
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
