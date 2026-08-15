---
task-type: implement
---
# R2f1b 3c2 Task A4 owner-authorized targeted repair

## Description

Perform the one owner-authorized targeted repair of the A4 candidate. The
frozen input is exact commit `04e5957949575bec053b0739b21d42dc670cbbcf`.
This is a bounded repair on the same artifact, not a restart; the delivered
owned-journal surface, candidate-API deletions, and A3 recovery-recheck
rider stay as shipped except where the three confirmed defects below require
change.

Own `crates/bridge-core/src/fs_custody.rs`,
`crates/bridge-core/src/namespace_transaction.rs`, focused colocated tests,
and `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
No other files. Do not change accepted A1-A3 behavior, wire Task B
consumers, or touch anything outside the Task A surface.

Implement exactly three repairs plus their fail-first tests:

1. **Coherent write-blocking debt semantics (confirmed WRONG).** Today the
   protective-debt flag is set only by the owned journal surface's
   `retained()`, never by namespace transaction `Retained`/`ProtectiveDebt`
   outcomes; `recover` refuses while the flag is set but has no path that
   clears it, permanently bricking the handle; and the flag is in-memory
   per-custody-handle only. Repair: route every mutating entry point's
   protective outcome (journal surface and namespace transactions) through
   one choke point that records debt on the custody handle; make `recover`
   clear the flag exactly when it completes a clean pass — empty reserved
   census, a fresh successful route proof, and a successful root sync —
   returning `Ready`; residue-backed debt continues to block via the census
   as today. Record in the handoff the accepted scope of the in-memory
   flag: residue-backed debt is durable via the residue itself across
   handles and restarts, while residue-free durability uncertainty
   self-heals through the next successful route-proof-plus-sync; that
   re-argument is submitted to the closure review.
2. **Per-operation capacity headroom (confirmed WRONG).** Admission
   currently accepts a census of exactly 4,096 entries and then creates
   more, after which every enumeration refuses and the root is permanently
   blocked. Repair: every mutating admission reserves its own maximum
   transient footprint (replace: stage plus intent; retire: intent; journal
   stage: one) and refuses with typed protective debt when the census plus
   footprint would exceed 4,096; an over-cap census in the journal guard
   classifies as `ProtectiveDebt`, not `Refused`. Recovery's bounded census
   behavior is unchanged.
3. **Reserved-prefix target rejection (confirmed WRONG).** A caller can
   today pass a target name with the `.a2a-v2-` reserved prefix; the
   mutation succeeds and every later guard and recovery pass classifies the
   published record as residue, permanently poisoning the root. Repair:
   every journal-surface and transaction entry point rejects a
   reserved-prefix target with a typed refusal before any filesystem
   effect. Parsing of actual residue is not weakened.

## Acceptance Criteria

- Begin with focused red tests; record the exact pre-change red commands and
  admissibility. Each repair needs at least one test that fails on the
  frozen input:
  - a namespace transaction ending `Retained` must leave the next journal
    mutator refusing with `ProtectiveDebt` on the same handle until
    `recover` completes a clean pass, after which mutation proceeds;
    degrading recover's clear-on-clean-pass must make a test red, and
    degrading the transaction-outcome debt recording must make a test red;
  - stage/replace/retire at a 4,095- and 4,096-entry census: admission
    refuses before creating any entry whenever the census plus the
    operation's footprint would exceed 4,096, and no test leaves an
    over-cap root behind;
  - every reserved namespace prefix and a generic `.a2a-v2-x` name are
    refused as targets by stage, publish, append, replace, and retire
    before any mutation, with the root byte-unchanged.
- Protective arms cannot flatten to success; no `rustfmt::skip` anywhere in
  the changed files' new code; normal formatting throughout.
- Existing owned-surface, A1-A3, and legacy tests keep passing unchanged
  except tests that pinned the three defective behaviors, which are
  corrected with cause recorded in the handoff.
- Run the focused selectors:
  `cargo test -p bridge-core --lib -- namespace_transaction custody_v2 fs_custody journal_route journal_surface`
  (adjust the final selector to the owned-surface test module's actual
  name).
- Run `git diff --check` and `cargo fmt --all -- --check`.
- Refresh the handoff: exact frozen input `04e59579`, red evidence, changed
  paths, the debt-scope re-argument, line accounting, and the statement
  that Task B and production V3 remain unarmed.
- Line caps, churn convention — additions plus deletions both count,
  measured post-format: stop and report before exceeding **140 changed
  production lines or 350 total changed lines** relative to `04e59579`.
  Do not reinterpret the convention.

## Files

- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-core/src/namespace_transaction.rs`
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the A1-A4 implementer statements)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): make journal debt coherent and admission self-safe

## Round Contract

This dispatch is the single owner-authorized targeted repair of the A4
artifact. One hard-read-only Sol/xhigh closure review follows separately; do
not self-repair a rejection. Never restart from a fresh artifact and never
extend the cap.
