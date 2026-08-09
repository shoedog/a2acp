# PARKED-1 no-replace rename classification — senior-lead review record

Date: 2026-08-09. Artifact: `fix/noreplace-errno-classify` @ `9d33c40a` → repaired
`4fb67368` (base `3d1fef9c`). Single lens (opus senior-lead) per the posture rule —
publication path, not byte-destroying. Verdict **SHIP, 0 WRONG, 9 SMELL/DEFER**; the top
five folded as an orchestrator repair on the branch, the rest ledgered below.

## What shipped

One shared mechanism rule, `classify_publication_rename_effect` (evidence order: staged
source present+ours → NotRenamed; target ours → Renamed; else Unverified; positive
identity matches on both sides), consumed by BOTH `fs_custody` publication primitives —
`ReplacePublicationV1` renamed to `CustodyPublicationV1` since publish and replace now
share one outcome lattice — and by the binary's `local_file` publication path
(signature preserved, message parity for the true-refusal arm pinned byte-exact). Two
additional pre-existing fail-open routes closed beyond the parked one: publish returning
`Err` after a successful rename when parent-sync or the post-rename identity recheck
failed.

## Review highlights (all verified against source by the reviewer)

- **`local_file` has four LIVE production callers** (admission-control, status-projection,
  notification, and outbox journals via `write_new_journal_record`) — the implementor's
  handoff understated this. The `Renamed`→`Ok` arm is a strict repair there: pre-change,
  an error-after-effect rename left memory behind disk and every subsequent append in the
  lock hold refused with `publication target already exists`. No caller retries (the
  subsystem pins `attempts == 1`, `retry_cap == 0`), none branches on message text.
- **The evidence ORDER had no pinning test** (strongest find): with both names ours (hard
  link inside the failure window) a target-first classifier reports `Renamed` and the
  caller attests `Durable` for a refused publication. Repair added the both-ours
  assertion to the shared-rule test plus corrected the three doc sites that
  mis-attributed the ordinary-`EEXIST` refusal to rule 2 (it is rule 1's).
- **§2c disclosure ENDORSED**: the removal-quarantine capture rename stays unclassified —
  its "removal failed" answer is TRUE when only the capture happened, a deterministic
  recovery arm consumes the residue, and the one wedge state is unreachable by
  construction. Wording note only.

## Ledger

- **2b2 (added obligation):** staged-source residue policy for the ambiguous publication
  arms of the `fs_custody` primitives — `RenameOutcomeUnverified` may leave the staged
  temp under its source name or not; the writer must define recovery ownership before it
  is the first production caller (opus S-9). Also: the shared fault countdown counts
  publishes AND replaces together — documented on the seam; 2b2 crash-matrix tests must
  count every rename when arming call N (opus S-6).
- **Accepted, recorded:** two independent u8 encodings of the fault enum on either side
  of the crate boundary (values never cross it); the stale
  `#[cfg_attr(not(test), allow(dead_code))]` on `local_file`'s now-live publish entry
  (pre-existing); Linux arm and real-NFS behavior unexecuted (the retried-RPC shape is
  modelled through the fault seam) — carried into the fold gate report.
