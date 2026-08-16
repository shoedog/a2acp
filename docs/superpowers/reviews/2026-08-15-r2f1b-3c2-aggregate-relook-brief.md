---
task-type: code-review
---
# R2f1b 3c2 aggregate re-look: captured-checkpoint recovery fix

## Description

Perform the single bounded re-review of the owner-authorized aggregate
repair: exact diff `50f3336e..85690adb` in this checkout. This is the
one re-look the aggregate repair contract declared; it is capped at one
pass with no repair loop inside it. Do NOT re-review the rest of the
3c2 line — the aggregate dual-lens round already adjudicated it
(A4/C/G second looks SUSTAINED; the Opus lens sustained
release/compat/rollback/authority); your scope is THIS fix and its
blast radius.

The blocker being fixed (your own aggregate finding): the
checkpoint-replace transaction's durable `Captured` window (intent
present, ordinary checkpoint renamed to its capture name, successor
unpublished) was unrecoverable because `open_base` ran
`authorize_checkpoint` before `NamespaceTransactionV2::recover`, and
the absent-checkpoint branch refused `Malformed` with no
transaction-awareness — permanently bricking the journal.

The delivered fix (production 206+15 in `remote_request_flight.rs`,
111+1 in `namespace_transaction.rs`, +52 handoff; in-container verify
fully green; advisory APPROVE with one non-blocking equal-length
commitment-test DEFER):

- the absent-checkpoint branch now: runs the residue-tolerant full-row
  `scan_with(op, true)` FIRST (the Task C validation-before-recovery
  property), then calls the new READ-ONLY
  `NamespaceTransactionV2::inspect_captured_replace_predecessor(op,
  name, cap, label)` which returns the captured predecessor bytes only
  for a valid single intent targeting the checkpoint name, then
  validates those bytes with the unchanged `validate_checkpoint`
  (attempt identity + digest), then invokes recovery, then re-runs
  `authorize_checkpoint` against the recovered namespace; every other
  absent-checkpoint state refuses `Malformed("checkpoint is absent")`
  byte-preserved;
- `scan_with` now returns `Option<CensusV1>` with a lease-checked
  tolerant absent-checkpoint arm and a `unique_children`
  ordinal/request-id collision check;
- red-first integrated crash-cuts via a new test-only
  `interrupt_replace_at_captured_for_test` hook: the REAL admission
  advancement and the REAL orphan-heal replacement interrupted at
  `TransitionV2::Captured` with the on-disk brick state asserted
  (ordinary checkpoint absent), then reopen recovers (checkpoint
  advanced; healed orphan `PreSendFailure`) and a second reopen also
  succeeds; foreign/corrupt-capture and multi-intent refusal cases.

Adjudicate:

- the blocker as FIXED, PARTIAL, or OPEN against `85690adb` — trace
  BOTH real call sites through the crash window and reopen;
- the trust root: does the inspection accessor introduce any authority
  the old path lacked? It must be read-only until every validation
  passes; a foreign/corrupt capture, commitment mismatch, multiple
  intents, or an intent targeting another name must refuse with NO
  recovery mutation (verify the namespace is untouched on refusal);
- the recursion in `authorize_checkpoint` (re-run after recovery)
  terminates: recovery either publishes the successor or restores the
  predecessor, so the second pass takes the ordinary branch — falsify
  if any recovery outcome leaves the name absent;
- the `scan_with` signature change: every caller handles `None`
  correctly; the strict (`tolerate_residue=false`) path still refuses
  an absent checkpoint; the Task C property holds in both paths;
- the advisory DEFER (equal-length commitment-test gap) — judge
  whether it hides a blocker; otherwise it joins the aggregate ledger;
- scope: only the two core modules, colocated tests, and the handoff
  changed; no production caller/route change; production V3 remains
  unarmed; no new `rustfmt::skip`; `Cargo.lock` unchanged.

Supplied exact-head evidence is corroboration only; you are licensed
to falsify or reject every supplied result: head `85690adb`, clean
worktree, branch `implement/impl-49324-q705iej1`; in-container verify
fully green; operator host gates on exact `85690adb` all exit 0: full
locked all-feature workspace test **4,104 passed / 0 failed / 13
ignored across 90 harnesses**, workspace clippy `-D warnings`, locked
release build, `cargo deny check`, repository hygiene.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must
  name a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the blocker, the trust root, the recursion
  bound, and the signature-change blast radius.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- `crates/bridge-core/src/namespace_transaction.rs`
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`
