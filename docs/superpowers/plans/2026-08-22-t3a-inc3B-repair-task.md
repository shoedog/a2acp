---
task-type: implement
---

# 3B repair — one mis-scoped path, and two degraded-worktree fixtures

## Description

Your increment 3B implementation is **accepted on design and evidence**. Both
reviewers independently confirmed it is a faithful match to an exacting spec —
the retained root threading, the Host Git brackets, the sixteen-row degraded
matrix, the stale rows, and the persisted-record controls are all right.

Three defects block it, all mechanical. **Fix only these.** Do not restructure,
do not change production logic, and do not weaken any assertion.

Your base is your own candidate.

### Important: two of these were invisible until the first one was fixed

The crate did not compile, so **no test in the diff had ever executed**. The
operator applied defect 1 locally to see what lay behind it, and two tests failed
on their first-ever run. Those are first-execution results, not regressions —
your design was never wrong, it was never exercised.

---

## Defect 1 — a mis-scoped path breaks compilation

`crates/bridge-worktree/src/sweep/checked_scan.rs`, in the test at ~line 945:

```rust
let super::ExactAbsenceRecordAssessmentV1::Custody(assessment) = &rows[0].assessment else {
```

`ExactAbsenceRecordAssessmentV1` is not in `super`. `[MEASURED]`
`cargo build --locked -p bridge-worktree --tests` gives exactly one error:

```
error[E0433]: failed to resolve: could not find `ExactAbsenceRecordAssessmentV1` in `super`
   --> crates/bridge-worktree/src/sweep/checked_scan.rs:945:20
```

The correct form is **two lines below it in the same test**, which already reads
`crate::sweep::CustodyExactAbsenceAssessmentV1`. Use `crate::sweep::` here too.

**Then run `cargo fmt --all`.** `[MEASURED]` the corrected line exceeds
`max_width`, so rustfmt breaks the `else` onto its own line. Fixing the path
without formatting leaves the fmt gate red.

---

## Defects 2 and 3 — the degraded-worktree fixtures violate a validate rule

Both failures are the same cause, in two tests:

- `degraded_claim_authority_matrix_has_sixteen_rows_and_only_worktree_degradation_probes`
- `persisted_record_degraded_worktree_reaches_host_git_and_preserves_bytes`

Both panic on `record.encode_canonical().unwrap()` with
`Err(ClaimIdentityMismatch)`.

**Root cause, measured.** `WorktreeCustodyRecordV1::validate` in
`crates/bridge-worktree/src/custody.rs` requires **exact structural equality**
between the record's envelope worktree and the claim's worktree:

```rust
if claim.custody_id != self.custody_id
    || claim.checkout_fingerprint != self.checkout_fingerprint
    || claim.current_attempt != self.current_attempt
    // This is exact structural equality between the duplicated envelope and claim bytes,
    // not filesystem verification; differing birthtime presence is contradictory here.
    || claim.worktree != self.worktree
{
    return Err(CustodyRecordDecodeErrorV1::ClaimIdentityMismatch);
}
```

Your fixtures degrade **only** the claim copy —
`record.claim.as_mut().unwrap().worktree.directory_identity` — and leave
`record.worktree` intact. The two copies then differ, and validate refuses.

The code comment states the rule outright: *differing birthtime presence is
contradictory here*. A record whose envelope claims a complete worktree identity
while its claim carries a degraded one is not a degraded record; it is an
inconsistent one.

**Fix:** when degrading the worktree identity, apply the identical degradation to
**both** `record.worktree` and `record.claim.worktree`, so the record is
internally consistent and genuinely degraded.

This affects the **worktree** degradation only. Degrading `source`, `root`, or
`common_dir` does not trip this rule — validate compares only `custody_id`,
`checkout_fingerprint`, `current_attempt`, and `worktree`. Do not change those
paths.

Do not weaken the assertions, and do not work around the rule by skipping
`encode_canonical` — the persisted-record tests must write bytes a real decoder
would accept, which is the point of entering through
`sweep_orphans_with_exact_absence`.

---

## Everything else is accepted

Do not modify: the retained root threading, the Host Git brackets, the sixteen-row
matrix's structure or expectations, the stale rows, the frozen control, any scope
fence, or any production file beyond what defect 1 requires (which is none —
defect 1 is in a test).

If a fix appears to require changing production logic or an assertion, stop and
report under the falsification license rather than widening scope.

## Handoff

Amend the existing 3B handoff in place. Record that the two fixture failures were
first-execution results behind a compile error, not regressions, and state the
validate rule that caused them so the next reader does not rediscover it.

**You make the implementation-candidate commit only**, with the work **staged**.
Gate execution and the handoff-only evidence commit belong to the host operator.

## Sizing

| Counted component | Estimate | Cap |
|---|---:|---:|
| Defect 1 path fix and its formatter break | 3 | 15 |
| Defects 2-3 fixture degradation of both copies | 12 | 40 |
| Handoff amendment | 15 | 35 |
| **Total** | **30** | **90** |

## Acceptance Criteria

1. `cargo build --locked -p bridge-worktree --tests` compiles with zero errors.
2. `cargo fmt --all -- --check` passes.
3. Both named tests pass, with their assertions unchanged in substance.
4. Worktree degradation applies identically to both `record.worktree` and
   `record.claim.worktree`; source/root/common-directory degradation is untouched.
5. No production logic changes; no assertion is weakened; `encode_canonical` is
   still used to write persisted-record bytes.
6. No other test changes colour.
7. The handoff is amended in place and records the validate rule.
8. The six `PENDING OPERATOR` lines remain unticked, and exactly one
   implementation-candidate commit exists with the work staged.

Do not claim any gate result. Do not tick a pending box.

## Files

- `crates/bridge-worktree/src/sweep/checked_scan.rs` — defect 1 only.
- `crates/bridge-worktree/src/sweep.rs` — the two fixtures only.
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc3b-handoff.md` — amend.
- Everything else — read-only.

## Spec Refs

- `crates/bridge-worktree/src/custody.rs` — `WorktreeCustodyRecordV1::validate`
  and the envelope/claim worktree equality rule.
- `docs/superpowers/plans/2026-08-21-t3a-inc3B-task.md` — the 3B spec; its scope
  fences and falsification license still apply.

## Commit Message

fix(worktree): scope a test path and make the degraded fixtures consistent

Resolve `ExactAbsenceRecordAssessmentV1` through `crate::sweep::` rather than
`super::`, which did not compile and therefore prevented every test in the slice
from executing.

Degrade both the envelope and claim worktree identities together. `validate`
requires exact structural equality between them, so degrading only the claim copy
produced an inconsistent record that `encode_canonical` refused, rather than a
degraded one.

## Falsification license

These measurements are operator claims against your base. The repository is
authoritative. If the compile error differs, if `validate` does not compare
`claim.worktree` to `self.worktree`, or if degrading both copies does not make the
fixtures pass, record the exact repository evidence and stop before editing.
