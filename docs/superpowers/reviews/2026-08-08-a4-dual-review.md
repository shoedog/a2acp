# A4 fs_custody extraction — dual-lens review record

Date: 2026-08-08. Artifact: `feat/a4-fs-custody-extraction` @ `6ca7b087` → repaired `f11aa7c0` (base
`49274e04`). Lenses: Opus/high senior-lead; gpt-5.6-sol/high via the bridge. One round + one combined
repair. This closes the closure-record-mandated `fs_custody`/`local_file` adjudication.

## Verdicts

- **Opus: REVISE (1 blocker + 4 smells).** Deletion-boundary parity verified exact in both reapers —
  including the two tempting non-moves it specifically tried to break (the preserved double-stat TOCTOU
  quirk; the bare identity check rather than the canonicalize variant). The blocker was real: the
  `ErrorKind::Unsupported` remap was reachable on the build matrix (std maps `ENOSYS`/`EOPNOTSUPP` to
  that kind; `renameatx_np` returns `ENOTSUP` on SMB/NFS/FUSE/exFAT), so real filesystem errors printed
  the platform message with the errno discarded — falsifying the slice's two-divergence claim.
- **Sol: REJECT on acceptance literalism (0 WRONG, 2 smells).** Parity confirmed at 96/100 across
  predicates, ordering, flags, mappings, countdown, bracketing, outcome projection. Rejected on the
  gate-line placement (report-construction timing, no constructible wrong output — its own grading) and
  missing `PayloadNotADirectory`/wrapper-message coverage. Adjudication: the smells were taken because
  they are cheap and good, not because the grade was right.

## Repair (`f11aa7c0`)

Typed `RenameNoReplaceRefusalV1{PlatformUnsupported, Io}` — the platform message now has exactly one
construction site behind `#[cfg(not(any(macos, linux)))]`; kernel refusals keep their errno text
(red-first against the real pre-fix code). The repair corrected the review's own mechanism detail
(Darwin: `ENOTSUP`(45) ≠ `EOPNOTSUPP`(102); only the latter decodes `Unsupported` — the first probe was
inadmissible and fixed before any belief update). `PayloadNotADirectory` boundary tests (file + symlink
swap, act-count == 0, symlink target intact); byte-exact wrapper-message tests incl. the both-rules
`a\0/b` tie-break; gate-line placement mirrored into the act closure with an ordering witness (honest
limit documented: the biconditional is asserted because the line is env-invisible); park-reason mapping
deduped with per-variant unit tests; act-runs-once counters. Divergence enumeration now truthfully two
entries. Final: fs_custody 36, whole-bin 1058, all storage filters green, 6/6 flake sweep.

## Carried

The descriptor-relative removal stays PARKED with its substitution seam documented in
`verify_then_remove`'s docstring (`ReapEnv::remove_tree` is the swap point). Process lesson recorded by
the implementor: mutation scripts must restore from a snapshot COMMIT (`git checkout HEAD --`), never
rely on the index across `stash pop`.
