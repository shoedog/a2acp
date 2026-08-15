---
task-type: implement
---
# R2f1b 3c2 Task A3 owner-authorized targeted repair

## Description

Perform the one owner-authorized targeted repair of the preserved A3
candidate. The frozen input is exact commit
`b1b55a218c0b78213ec4a719ab96831cd766bd87` (the mechanical reformat of
rejected candidate `f6b6ccf6`; the module is now normally formatted with no
`rustfmt::skip`). This is a bounded repair on the same artifact, not a
restart, not a rewrite, and not an A4 or Task B step.

Own `crates/bridge-core/src/namespace_transaction.rs`, narrow
`crates/bridge-core/src/fs_custody.rs` mechanism changes only where the typed
error preservation requires them, `crates/bridge-core/Cargo.toml` only if a
digest dependency edge is needed, focused tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not change the accepted A1/A2 public behavior, the shared generation
journal, worktree custody, `local_file`, reapers, recursive removal, or
anything outside the named files. Production V3 stays unarmed; the module
keeps zero production callers.

Implement exactly four repairs:

1. **Typed `Unsupported` preservation (confirmed WRONG).** The transaction
   mechanism currently stringifies every `FsCustodyError`, so
   missing-birthtime identity surfaces as `Retained` recovery debt with no
   recoverable residue, and a runtime `RENAME_NOREPLACE` refusal (`ENOTSUP`
   filesystems) surfaces via recovery as `NoEffect`. Repair: preserve the
   typed custody error through `snapshot` and every mechanism path; classify
   unsupported identity/primitive as
   `NamespaceTransactionOutcomeV2::Unsupported`. Where the incapacity is
   knowable before any mutation (the pre-stage target snapshot in both
   replace and retire), refuse before stage/intent creation. A runtime
   refusal first observable at capture must roll back its own stage/intent
   residue and still return typed `Unsupported`, never `NoEffect`, `Retained`,
   or a recovery loop. `recover` on such a filesystem likewise surfaces
   `Unsupported`.
2. **Staged content commitment (owner-ruled design amendment).** Add a
   bounded SHA-256 commitment of the staged successor bytes to the immutable
   intent (wire note: no persisted production intents exist anywhere, so the
   intent wire may keep schema 2 or bump; `deny_unknown_fields` stays; there
   is no compatibility obligation). Wherever `FileContentSnapshotV2` equality
   currently stands in for content equality on a path that can end in
   `Complete` for a replace — post-publication target verification and every
   recovery arm that classifies the target as the staged successor — read the
   bytes and verify them against the commitment. Mismatch classifies
   `Retained` with the captured predecessor preserved, never `Complete` or
   `NoEffect`. Retirement needs no commitment: its `Complete` asserts exact
   predecessor removal, which the held-descriptor zero-link proof already
   establishes; record that reasoning in the handoff. Use a digest
   implementation already present in `Cargo.lock` (for example `ring`); a new
   manifest edge is allowed, but the lock must not gain new packages — every
   gate runs `--locked`.
3. **Rider-test hardening (review SMELL-1 subset).** Make the same-cell
   mutex test deterministic: the second thread must be proven to have entered
   `begin_operation` and to remain blocked until the first guard drops, via a
   channel/barrier ordering, not a pre-call signal. Extend the retire crash
   matrix to every cut (intent sync, capture, retirement unlink, zero-link
   proof, intent removal, final sync) with pinned recovery outcomes matching
   the replace matrix's protective semantics; where a post-retirement cut
   pins permanent protective `Retained` debt, pin it explicitly and record in
   the handoff that residue-disposition authority is a later-slice ledger
   item, not silently relaxed here.
4. **Formatting discipline.** The change stays normally formatted; no
   `rustfmt::skip` anywhere in production or tests of the changed files.

## Acceptance Criteria

- Begin with focused red tests. Record the exact pre-change red commands and
  why each observation is admissible; each of the following must fail on the
  frozen input and pass after the repair:
  - missing-birthtime target identity refuses typed `Unsupported` before any
    stage or intent exists (assert the namespace is unchanged);
  - an injected `ENOTSUP`-class capture refusal returns typed `Unsupported`
    with stage/intent rolled back (assert no reserved residue remains and no
    `NoEffect`/`Retained` is returned);
  - the exact adjudicated corruption scenario: crash after publication
    (successor at target, predecessor in swap, intent present), rewrite the
    target in place to same-length different bytes, then recovery returns
    `Retained` with the predecessor still present in swap — never
    `Complete`;
  - a commitment mismatch at post-publication verification refuses
    publication success on the live path as well.
- The deterministic mutex-queuing and full retire crash-cut regressions from
  repair item 3 are present; the mutex test proves blocking-until-release by
  construction.
- All existing `namespace_transaction`, `custody_v2`,
  `journal_route_custody_v2`, and legacy `fs_custody` tests continue to pass;
  accepted A1/A2 public behavior is unchanged.
- Run the focused selectors:
  `cargo test -p bridge-core --lib -- namespace_transaction custody_v2 fs_custody`.
- Run `git diff --check` and `cargo fmt --all -- --check`.
- Refresh the handoff: exact frozen input `b1b55a21`, red evidence, changed
  paths, production/test line counts, the retirement-commitment reasoning,
  the residue-disposition ledger note, and the explicit statement that A4,
  Task B, and production V3 remain unarmed.
- Stop and report before exceeding **150 changed production lines or 350
  total changed lines** relative to `b1b55a21`, measured under normal
  formatting. Do not solve a cap breach by weakening a protective arm or
  deleting inherited tests.

## Files

- `crates/bridge-core/src/namespace_transaction.rs`
- `crates/bridge-core/src/fs_custody.rs` (narrow, typed-error paths only)
- `crates/bridge-core/Cargo.toml` (digest edge only if needed; lock gains no
  new packages)
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the A1-A3 implementer statements)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): type unsupported custody outcomes and commit staged content

## Round Contract

This dispatch is the single owner-authorized targeted repair of the A3
artifact. One hard-read-only Sol/xhigh closure review follows separately; do
not self-repair a rejection. Never restart from a fresh artifact and never
extend the cap.
