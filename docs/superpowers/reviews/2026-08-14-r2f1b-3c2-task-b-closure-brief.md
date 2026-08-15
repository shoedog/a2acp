---
task-type: code-review
---
# R2f1b 3c2 Task B1 closure review

## Description

Perform the one declared hard-read-only closure review of the complete Task
B1 line: exact diff `d8ec93ad..6033fd34` in this checkout, where `d8ec93ad`
is the accepted Task A head and `6033fd34` is the current head. Review the
full diff, the complete new module, and its consumption of the Task A
surfaces in context. Do not edit, build, test, invoke another provider, or
access the network. This review round is capped at one; no repair loop is
authorized inside it.

The line contains three commits:

1. `2815259d` — the B1 implementation: request child/checkpoint grammar,
   atomic admission (checkpoint validation, bounded census, capacity
   refusal before mint, checked ordinal allocation, temp-sync-rename-sync
   initial publication, checkpoint advance, authority only after both
   publications), strict decoding, and the authorized B1/B2 split
   (retirement named as the B2 remainder). Its advisory review found six
   closed blockers, all operator-verified at source.
2. `02a14298` — the one declared targeted repair, all six fixes delivered
   (the advisory reviewer of this commit confirmed each): unforgeable
   authority (private field, no `Clone`/`Copy`, single construction site in
   the admission tail), a private strict Serde remote wire for the nested
   `AttemptIdentity` with `deny_unknown_fields`, duplicate-mint refusal
   before staging, exact enumeration-limit overflow classified `Capacity`,
   the clippy fix, plus protective-outcome injection at a real Task A
   boundary and the positive-edge `next_ordinal` assertion. Production
   additions vs `d8ec93ad` measure exactly 500 (498 module + 2 export) —
   at, not over, the B1 cap.
3. `6033fd34` — an operator docs-only correction of the handoff accounting
   (production churn `+43/-38`, the 381 test-only module additions
   recorded) and the recorded duplicate-mint policy: a repeated CSPRNG
   identity means the identity source is suspect, so the handle
   deliberately requires reopen afterward — fail-closed by policy, not
   oversight.

Operator adjudications you must independently judge:

1. The in-container verify red on both B1 runs was the whole-bin
   `a2a-bridge` harness with the flock-EBADF signature
   (`authority-state.lock`/`owner-admission.lock`, os error 9) — the fourth
   instance of the lane's recorded hermetic class; the exact heads are
   host-green (totals below). Judge the classification; the exact failing
   harness is untouched by this diff.
2. The duplicate-mint fail-closed reopen policy (the advisory SMELL): judge
   whether freezing the handle after a catastrophic identity collision is
   sound protective policy for a B2/C consumer, or names a bounded change.
3. The B1/B2 split: retirement and reopen self-healing are explicitly named
   B2 scope; judge that nothing shipped forecloses them.

Required judgments:

- Admission atomicity: no zero-row reservation at any crash cut; authority
  returned only after both publications; step-5 (checkpoint advance)
  failure returns no authority and reopen closes the orphan child as a
  pre-send failure.
- Protective consumption: only exact `Complete` from the Task A surfaces
  advances the checkpoint; every protective or refused Task A outcome
  blocks with a typed refusal and no flattening.
- Capacity: refusal before ID mint at the bound; the exact
  enumeration-limit overflow is `Capacity`; admission footprint accounted.
- Strict decoding end-to-end, including the nested attempt identity;
  legacy/foreign/corrupt roots refuse without mutation.
- The sealed authority: no public constructor, no duplication path, single
  production construction site.
- Scope: the module is unreachable outside tests (only the `lib.rs` export
  and colocated tests reference it); Task A surfaces unchanged; no
  production caller, route, persistence encoding, or V3 arming;
  `Cargo.lock` unchanged; normally formatted, no `rustfmt::skip`.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `6033fd34`, clean worktree, branch `implement/impl-56580-abz3axmg`;
- cumulative B1 diff vs `d8ec93ad`: new module 879 lines (498 production +
  381 test), `lib.rs` +2, handoff +57;
- focused suites at the repair head: `remote_request_flight` 8/0; the
  combined Task A/B1 selectors 126/0;
- operator host gates on exact `6033fd34` all exit 0: `git diff --check`,
  formatter, locked all-target/all-feature workspace check and Clippy with
  `-D warnings`, full locked all-feature workspace test **4,034 passed / 0
  failed / 13 ignored across 90 harnesses**, locked release build,
  `cargo deny check`, and repository hygiene 40 tracked / 8 configs.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must name
  a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the six round-1 findings as FIXED, PARTIAL, or OPEN
  against the shipped line, and the accounting WRONG as resolved or not.
- Judge the three operator adjudications.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- `crates/bridge-core/src/lib.rs`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the B1 implementer statements)
- repository `AGENTS.md`
