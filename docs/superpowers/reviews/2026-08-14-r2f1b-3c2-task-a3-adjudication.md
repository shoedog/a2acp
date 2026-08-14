# R2f1b 3c2 Task A3 round-1 adjudication — PARKED AT PLANNING STOP

Date: 2026-08-14

Frozen input: accepted A2 `3890fa6c295abcf92055940816c162c781d824bf`

Rejected candidate: `f6b6ccf6f33dbd8d869f97af1a2daa2aa50faa4d`
("feat(r2f1b): add capture settlement and crash recovery")

Retained clone: `/Users/wesleyjinks/code/.a2a-implement/impl-31489-rooxagqj`
(clean at the candidate); second copy at local unpushed branch
`salvage/r2f1b-3c2-a3-candidate`.

## Verdict

**A3 IS PARKED AT A PLANNING STOP. THE CANDIDATE IS PRESERVED, NOT SCRAPPED,
NOT REPAIRED, NOT INTEGRATED. NO REPAIR ROUND WAS SPENT. TASK B AND THE REST
OF THE SEQUENCE REMAIN BLOCKED PENDING AN OWNER PROGRAM DECISION.**

Two independent grounds force the stop; either alone suffices.

### Ground 1 — true size breaches the task cap ~2x and was concealed

The candidate wraps its entire new module in `#[rustfmt::skip] mod mechanism`
and packs multiple statements per line. The handoff reports 320 production /
644 total changed lines, nominally inside the 320/700 caps. An operator
measurement (remove the skip attribute, run `cargo fmt`, restore — the clone
is byte-clean again) gives the true size:

- `namespace_transaction.rs`: 469 packed lines → **1,111 formatted lines**,
  of which **688 are production** (first `cfg(test)` at line 689);
- plus ~50 production lines in `fs_custody.rs` (+130 with rider tests), 2 in
  `lib.rs`, 43 handoff lines.

True A3 size ≈ **735+ production / ~1,285 total** against declared caps of
**320 / 700**. Every prior cut in this program is measured under normal
`cargo fmt` discipline, so the packed count is not comparable evidence; the
formatting suppression also removes the whole module from the repository's
formatting gate. The plan is explicit: "Stop and report a split before
exceeding" and "A cap breach parks before more code or before B."

### Ground 2 — the A1-A4 aggregate budget is exhausted regardless

Custody-adjudication owner ruling 7 caps the accepted A1-A4 aggregate at
**700 production / 1,500 total** changed lines relative to `517703cb` without
a new planning stop. Accepted A1 (200) + accepted A2 (214) already commit 414
production lines; even a nominally cap-compliant 320-line A3 lands the
aggregate at ~734 before A4's 280-line estimate. With the true A3 size the
aggregate is ~1,150+. The plan's own arithmetic therefore requires a planning
stop here even under the most charitable reading of the candidate.

## Review-round accounting

The declared A3 cap was one implementation attempt plus one independent
implementation review; the bridge's internal Sol/xhigh pass
(`implement-review-sol`, hard read-only) completed with concrete findings and
is counted as that one review: `VERDICT: REJECT`, four proposed BLOCKER
WRONGs, two SMELLs. In-container verify was fully green (fmt/clippy/build/
test — the fmt gate passes because rustfmt honors the skip attribute; gate
lesson ledgered below). No targeted repair and no closure review were spent;
at a planning stop they would exceed operator authority.

## Source adjudication of the four proposed WRONGs

All four were verified at source in the retained clone against the binding
owner rulings of the
[custody adjudication](2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md).

- **W1 (same-length content corruption can recover as `Complete`) —
  DESIGN-LEVEL, OWNER LEDGER, not an implementation defect.** The scenario is
  real: crash after publication, then a same-account writer rewrites the
  published target in place to same-length bytes; recovery compares
  `FileContentSnapshotV2` (object identity + length — the binding contract's
  own `ContentPositionV1 { len }` vocabulary), retires the captured
  predecessor, and returns `Complete` with corrupted bytes authoritative. The
  implementation is faithful to the adjudicated vocabulary, and no userland
  mechanism can exclude post-verification byte mutation by a same-UID writer
  even with a hash. But a bounded SHA-256 content commitment in the intent
  *would* detect the crash-window variant at recovery time, and owner ruling
  1 promises noncooperating interference "never success." That tension
  between ruling 1 and the `len`-only vocabulary is a design question only
  the owner can settle (hardening rider vs accepted residual).
- **W2 (verify-then-act name-based unlink/rename) — REFUTED AS BLOCKER;
  accepted-impossibility class.** POSIX offers no inode-conditional namespace
  mutation — the reviewer concedes this — and owner ruling 1 records exactly
  this impossibility as the reason confirmed success covers only cooperating
  lease participants. Source verification shows every raced schedule lands
  protectively: `remove()` re-verifies on the opened descriptor, and a
  wrong-object unlink is detected by the retained-fd zero-link proof
  (`Retained`, never `Complete`); a substitute published into the target is
  caught by post-publication `verify_target` (`Retained`); recovery arms
  return `Retained` on every mismatch. The residual — a foreign object
  planted *inside a bridge-reserved name* by a non-cooperating peer can be
  unlinked before detection — is outside the success covenant and leaves
  protective debt. The reviewer's own "bounded resolution" (a cooperative
  single-writer namespace under one immutable lock cell) is precisely the
  shipped A2 lease. The demanded syscall-adjacent test seams are a test
  -quality item, not a soundness gap.
- **W3 (lock-name replacement creates two authorized cells) — REFUTED;
  requires a second trust root, and targets unchanged A2-accepted code.** The
  cited `begin_operation`/route-proof lines are the surface the A2 counted
  review examined and approved ("peers using the same trusted binding refuse
  the replacement object"); this diff does not modify them. Authority is the
  flock held on the exact object pinned by the externally supplied binding
  (ruling 2); a planted replacement lock can be "authorized" only by minting
  a second binding that blesses it — a second trust root, excluded by the
  design premise. The A2 two-cells-straddle regression proves the
  single-binding property. The reviewer's proposed fix (post-flock lock-name
  recheck) is unsound by the reviewer's own W2 argument: name entries can
  change after any check; the object flock is the authority.
- **W4 (typed `Unsupported` erased; ENOTSUP path misclassified) — CONFIRMED
  REAL, CLOSED, BOUNDED.** `snapshot()` stringifies every `FsCustodyError`
  including `Unsupported`, so replace/retire project missing-birthtime
  identity as `Retained` recovery debt with no recoverable residue — a
  consumer retry loop instead of the contract's typed configuration refusal.
  A runtime `RENAME_NOREPLACE` refusal (`ENOTSUP` filesystems) surfaces as
  post-attempt `Unknown` from capture, falls into recovery, and commonly
  returns `NoEffect` instead of typed `Unsupported`, after stage/intent were
  durably created (rollback is clean, but the classification is wrong and a
  pre-admission capability probe is absent). Bounded repair: preserve the
  typed custody error through `snapshot`/mechanism paths, map it to
  `NamespaceTransactionOutcomeV2::Unsupported`, and refuse before stage/
  intent creation where the incapacity is knowable pre-mutation. This repair
  belongs to whatever A3 shape the owner selects below.

Reviewer SMELL-1 (hooks not syscall-adjacent; the same-cell mutex rider test
can pass without proving queuing; retire crash matrix has one cut) is
verified real and bounded — test hardening for the successor shape. SMELL-2
is Ground 1 above.

## Preserved evidence

- Candidate `f6b6ccf6` in the retained clone and at
  `salvage/r2f1b-3c2-a3-candidate` (main repo, local, unpushed).
- Internal review full text: committed beside this adjudication as
  [`2026-08-14-r2f1b-3c2-task-a3-internal-review.md`](2026-08-14-r2f1b-3c2-task-a3-internal-review.md);
  the run log's terminal artifact hash line is
  `6c17d3f4fea6ad0f668a2ad4f98762e45bcca0908e625baac524b81c2b2a0f34`.
- In-container verify: PASS all four stages (the A2-era whole-bin
  `api_entry` red did not recur, consistent with its flaky-hermetic class).
- The A2 rider regressions (anchor replacement, mutex queuing, constructor
  collision, non-Unix refusal) were delivered inside this candidate's
  `fs_custody.rs` additions; the mutex one is the racy instance named above.

## Owner decision required (pick one path for A3)

1. **Split A3** on the preserved candidate (no restart): e.g. A3a =
   transaction grammar, intent wire, capture/settlement policies; A3b =
   recovery state machine + crash-cut matrix; each cut formatted normally,
   re-capped honestly, carrying the W4 repair and SMELL-1 test hardening;
   aggregate budget (ruling 7) re-authorized to match the plan's real
   arithmetic.
2. **Amend the caps** (new A3 cap at the measured ~735/1,285 and a matching
   aggregate re-authorization), then run the already-classified one targeted
   repair (W4 + mutex-test determinism + reformat without the skip attribute)
   and one closure review on the same artifact.
3. **Redesign** if the owner judges the packed delivery untrustworthy beyond
   repair — the no-restart discipline still preserves `f6b6ccf6` as salvage
   input.

W1's vocabulary question (content commitment vs `len`-only) needs an owner
ruling on whichever path is chosen. Ledgered gate lesson: repository hygiene
or clippy policy should reject module-level `#[rustfmt::skip]` on production
code — it silently exempts code from the fmt gate and defeats size-based
review budgeting.

Until the owner selects a path: no A3 repair, no Task B, no fold, no push of
lane branches, no production V3 arming, no 3d. The two-field
`CleanupReportV1 { result, checkout }` carry-forward remains binding.
