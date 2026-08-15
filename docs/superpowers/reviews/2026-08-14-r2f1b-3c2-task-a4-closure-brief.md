---
task-type: code-review
---
# R2f1b 3c2 Task A4 owner-authorized closure review

## Description

Perform the one declared hard-read-only closure review of the complete Task
A4 line: exact diff `6114596d..7a973866` in this checkout, where `6114596d`
is the accepted A3 head and `7a973866` is the current head. Review the full
diff, the complete changed files, and the whole owned journal surface in
context. Do not edit, build, test, invoke another provider, or access the
network. This review round is capped at one; no repair loop is authorized
inside it.

The line contains three commits, each previously adjudicated with owner
authorization:

1. `04e59579` — the A4 implementation: the owned journal API (stage,
   publish, append, sync through `JournalRootOperationV2` with owned write
   sessions and no raw `File`/path escapes), write-blocking debt, deletion
   of the candidate V1 APIs (`revalidate`-as-authority, the path-exposing
   persistent-lock surface, raw writable-file/replacing-rename/name-unlink
   methods and `JournalRootCustodyV1` where unreferenced), and the A3
   closure rider (the recovery-path commitment recheck regression). Its one
   independent Sol/xhigh review REJECTed with five claims; operator source
   adjudication confirmed three (debt-flag mechanics, capacity headroom,
   reserved-target self-poisoning), refuted the route check-vs-syscall
   claim as the third instance of the accepted-impossibility class under
   the cooperating-participant ruling, and the owner regularized the
   measured size (499 production / 1,010 total churn — the ~500 deletion
   lines are the mandate itself).
2. `6a6ea1f9` — the owner-authorized targeted repair (342 total churn,
   within its 140/350 declared caps): debt recording choke point with
   recover-clears-on-clean-pass (empty reserved census + fresh route proof
   + root sync), per-operation capacity footprints against the 4,096 bound
   with over-cap census classified protective, reserved-prefix target
   refusal, and boundary tests. Its advisory review found one remaining
   WRONG: recorded debt could still surface as ordinary `Refused` (or
   transaction `NoEffect`) when reserved-target validation or fallible
   route/census preflights ran before the debt check.
3. `7a973866` — a disclosed operator completion (259 churn, ~35
   production), red-first (three domination tests failed on the pre-change
   tree): recorded debt is now checked before reserved-target refusal and
   every fallible preflight (dedicated `refuse_debt` first in stage,
   publish, append; debt-first line in `guard`, which `sync` and the
   transaction `ready` path use); namespace transaction outcomes record
   debt at the engine (`*_with`) layer so hook-driven test paths exercise
   recording; the reserved-target checks in replace/retire now run after
   admission. One semantic repin, disclosed for your judgment: when a
   reserved-named OBJECT is present in the root, admission now refuses
   protectively (it is residue by definition) and the pure name-level
   `NoEffect`/`Refused("reserved target")` applies to the clean-root case;
   the prior pin expected the name refusal even with the object present.

Operator adjudications you must independently judge (you are licensed to
contest each; the threat model is the custody adjudication's
cooperating-participant lease ruling, sustained by the A2 and A3 counted
reviews):

1. The route check-vs-syscall claim (W2 of the A4 round) was refuted as the
   accepted-impossibility class: POSIX offers no inode-conditional
   namespace mutation, the lease is the covenant, and raced schedules land
   protectively.
2. The debt semantics as completed: residue-backed debt is durable via the
   residue itself across handles and restarts; the in-memory flag covers
   residue-free protective outcomes on the live handle; residue-free
   durability uncertainty self-heals through the next successful
   route-proof-plus-sync; `recover` clears the flag only on a clean pass.
   Judge whether any consumer decision inside B-G scope can go wrong under
   these semantics.
3. The reserved-object-present repin described above.
4. Cap accounting: A4 measured size owner-regularized; the repair and
   completion stayed within declared/disclosed bounds; judge only silent
   scope, not size.

Required judgments:

- The debt-domination invariant: after any protective outcome on a handle,
  every mutating entry point (stage, publish, append, sync, replace,
  retire) returns typed protective debt — never ordinary `Refused`,
  `NoEffect`, or success — until `recover` completes a clean pass; and
  recovery remains callable throughout.
- The deletion inventory: the candidate V1 APIs are gone with zero
  remaining references, no production caller was broken, and lock-fd
  privacy is fully restored (no public path, fd, or `File` projection
  anywhere in the Task A surface).
- Capacity: every mutating admission reserves its transient footprint
  against the 4,096 bound; no path can create an over-cap root; over-cap
  census classifies protective.
- Reserved-prefix targets can never mutate the namespace; residue parsing
  is not weakened.
- The A3 closure rider regression is present and discriminating.
- Accepted A1-A3 behavior is otherwise unchanged; the recovery table and
  commitment verification are intact; `Cargo.lock` unchanged; no
  production caller, route, persistence encoding, or V3 arming exists;
  everything is normally formatted with no new `rustfmt::skip`.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `7a973866`, clean worktree, branch `implement/impl-25502-s3b2uf5v`;
- cumulative A4-line diff vs `6114596d`: `fs_custody.rs` +505/−503,
  `namespace_transaction.rs` +417/−31, handoff +51;
- red-first receipts: three debt-domination tests failed on `6a6ea1f9`
  (retained log `a4c-red.log`): reserved-on-debt returned `Refused`,
  route-loss-on-debt returned `Refused`, and hook-driven transaction
  `Retained` left the debt flag unset;
- full `bridge-core --lib` suite at head: **610 passed / 0 failed**;
- operator host gates on exact `7a973866` all exit 0: `git diff --check`,
  formatter, locked all-target/all-feature workspace check and Clippy with
  `-D warnings`, full locked all-feature workspace test **4,024 passed / 0
  failed / 13 ignored across 90 harnesses** (ignored = the declared
  authenticated/live set), locked release build, `cargo deny check`, and
  repository hygiene 40 tracked / 8 configs;
- the in-container verify on both bridge runs of this line was fully green;
  the lane's earlier whole-bin flock-EBADF reds remain the recorded
  hermetic class.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must name
  a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the A4 round's confirmed findings (debt mechanics,
  capacity headroom, reserved-target self-poisoning, debt domination) as
  FIXED, PARTIAL, OPEN, or ACCEPTED-RESIDUAL against the shipped line.
- Judge the four operator adjudications and the reserved-object-present
  repin.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-core/src/namespace_transaction.rs`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the A1-A4 implementer statements)
- repository `AGENTS.md`
