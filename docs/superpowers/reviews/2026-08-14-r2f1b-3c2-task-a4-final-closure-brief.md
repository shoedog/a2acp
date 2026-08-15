---
task-type: code-review
---
# R2f1b 3c2 Task A4 final closure review

## Description

Perform the one owner-pre-authorized final closure review of the complete
Task A4 line: exact diff `6114596d..863f2fd4` in this checkout, where
`6114596d` is the accepted A3 head and `863f2fd4` is the current head. This
is the closure of the pre-authorized additional repair round; it is capped
at one pass with no repair loop inside it.

The prior closure (of head `7a973866`) rejected with exactly one BLOCKER
WRONG and adjudicated everything else FIXED or ACCEPTED-RESIDUAL: direct
journal `stage`/`publish`/`append` returned ordinary
`Refused("reserved target")` before consulting the on-disk reserved census,
so a fresh handle over a residue-bearing root misclassified protective state
as an ordinary caller refusal; and `publish` whitelisted the staging name it
derived from a reserved target. Two SMELLs were deferred (compile-only
initial owned-surface red evidence; the missing replace 4,094
positive-boundary case).

The new final commit `863f2fd4` (+68/−15, one file) implements exactly the
prescribed repair, red-first (the repinned object-present cases and the
derived-staging-only publish case failed on `7a973866`; log retained):

- `stage`, `publish`, and `append` now run `refuse_debt`, then the
  admission census (`guard`), then the name-level reserved refusal — so an
  on-disk reserved object classifies `ProtectiveDebt` before any name
  refusal, matching the namespace-transaction ordering;
- `publish` no longer whitelists the derived staging name when the target
  itself is reserved, so a root containing only that derived staging object
  classifies protectively;
- clean-root reserved-name requests still return
  `Refused("reserved target")` with no filesystem effect and the debt flag
  proven clear.

Adjudicate:

- the prior closure's WRONG 1 as FIXED, PARTIAL, or OPEN against
  `863f2fd4`, including the publish whitelist case;
- that the repair introduced no regression in the debt-domination
  invariant, capacity headroom, recovery clearing, or the clean-root
  refusal semantics;
- that the two previously deferred SMELLs remain deferred or name any new
  WRONG;
- scope: the final commit touches only `fs_custody.rs`; the cumulative line
  touches only the two owned Rust files plus the handoff; no production
  caller, route, persistence encoding, or V3 arming exists; production
  still assigns `resource_flight_route_v3 = None`.

All prior-line adjudications (cooperating-participant threat model,
owner-regularized sizes, the reserved-object repin, debt-durability scope)
were sustained by the prior closure and are not reopened unless you find a
new constructible WRONG.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `863f2fd4`, clean worktree, branch `implement/impl-25502-s3b2uf5v`;
- full `bridge-core --lib` suite at head: **610 passed / 0 failed**;
- operator host gates on exact `863f2fd4` all exit 0: `git diff --check`,
  formatter, locked all-target/all-feature workspace check and Clippy with
  `-D warnings`, full locked all-feature workspace test **4,024 passed / 0
  failed / 13 ignored across 90 harnesses**, locked release build,
  `cargo deny check`, and repository hygiene 40 tracked / 8 configs.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must name
  a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the prior WRONG 1 and confirm no regression in the
  previously FIXED families.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-core/src/namespace_transaction.rs`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`
