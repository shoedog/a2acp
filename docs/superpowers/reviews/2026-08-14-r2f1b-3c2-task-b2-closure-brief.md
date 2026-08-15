---
task-type: code-review
---
# R2f1b 3c2 Task B2 closure review

## Description

Perform the one declared hard-read-only closure review of the complete Task
B2 line: exact diff `6033fd34..2e472a09` in this checkout, where `6033fd34`
is the accepted B1 head and `2e472a09` is the current head. Review the full
diff and the complete module in context. Do not edit, build, test, invoke
another provider, or access the network. This review round is capped at
one; no repair loop is authorized inside it.

The line contains three commits:

1. `6115c93e` — the B2 implementation: acknowledged-retirement grammar
   (strict decoding preserved), identity-checked retirement freeing
   capacity, reopen self-healing for the two prescribed restart schedules,
   the beyond-capacity sequential throughput proof, and the two B1-closure
   riders (Task A fault coverage, owner validation). Its advisory review
   confirmed scope and grammar but found three blockers.
2. `09a19025` — the one declared targeted repair for the two
   operator-confirmed blockers: reopen now heals exactly one proven orphan
   (the child at `checkpoint.next_ordinal`); active children below the
   checkpoint stay untouched and active until Task C's durable send rows
   exist; gapped/duplicate/ahead censuses refuse protectively; and `open`
   validates the checkpoint (schema, digest, attempt identity)
   non-mutatingly before any Task A recovery, refusing
   ForeignAttempt/Malformed with byte-identical roots. The third round-1
   claim (permanent protective `Retained` after a mid-retire crash) was
   operator-refuted as a B2 blocker: it is the accepted A3 semantics,
   pinned by A3's own crash-cut tests and already on the owner ledger as
   the residue-disposition-authority question; B2 adds coverage that
   reopen surfaces it as a typed protective refusal without mutation.
3. `2e472a09` — a disclosed operator completion (+43/−21), red-first (the
   new side-effect assertions failed on the pre-call seams): stage,
   acknowledgement replacement, and orphan-checkpoint healing now consume
   the real adapter results through the wrap-actual injection seam (the
   same pattern the repair established for publish/retire), so an adapter
   regression can no longer stay green; the three prescribed clippy lints
   fixed; the `request_paths` test helper narrowed to published children
   so legitimate reserved residue is not miscounted.

Operator adjudications you must independently judge:

1. The mid-retire permanent-`Retained` refutation described above (the
   Task A scope shield forbids the reviewer-proposed Task A amendment in
   this lane; the residual is owner-ledgered).
2. The repair-round SMELL that below-checkpoint active children are
   intrinsically ambiguous was accepted as Task C scope by both the
   advisory reviewer and the operator: B2 deliberately refuses to guess.
3. The in-container verify red on the B2 runs was the whole-bin
   `a2a-bridge` flock-EBADF hermetic class again (fifth and sixth lane
   instances, same signature, untouched harness); the exact head is
   host-green (totals below).

Required judgments:

- Reopen healing is exactly scoped: one proven orphan at
  `checkpoint.next_ordinal`; issued-but-unacknowledged children are never
  relabeled; gapped/duplicate/ahead censuses refuse with preserved bytes;
  healing is idempotent.
- Authorization precedes mutation: no Task A recovery or any other
  mutation before the checkpoint's attempt identity is proven;
  foreign/malformed roots refuse byte-identically.
- Acknowledged retirement: only exact `Complete` acknowledges; retirement
  frees capacity; the ack-persisted crash retires without republishing; a
  crash after unlink leaves no debt; the sequential throughput proof is
  genuine.
- The fault-injection rider now exercises the production adapters for
  stage, publish, replacement, and retirement, with side-effect assertions
  that fail if an adapter is bypassed.
- Owner validation refuses empty/oversized/control-character owners before
  mint and during census with preserved bytes.
- Scope: only the authorized module and handoff changed across the line;
  Task A surfaces and `Cargo.lock` byte-unchanged; no production caller,
  route, persistence consumer, or V3 arming; normally formatted, no
  `rustfmt::skip`.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `2e472a09`, clean worktree, branch `implement/impl-9079-5czgier2`;
- cumulative B2 diff vs `6033fd34`: module churn +~590/−~65 plus the
  handoff; the repair measured 99 production churn against its 150 cap and
  the completion +43/−21 (verify independently);
- full `bridge-core --lib` at head: **631 passed / 0 failed**;
- operator host gates on exact `2e472a09` all exit 0: `git diff --check`,
  formatter, locked all-target/all-feature workspace check and Clippy with
  `-D warnings`, full locked all-feature workspace test **4,045 passed / 0
  failed / 13 ignored across 90 harnesses**, locked release build,
  `cargo deny check`, and repository hygiene 40 tracked / 8 configs.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must name
  a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the three round-1 findings and the two repair-round
  findings (clippy, injection rider) as FIXED, PARTIAL, OPEN, or
  ACCEPTED-RESIDUAL against the shipped line.
- Judge the three operator adjudications.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the B1/B2 implementer statements)
- repository `AGENTS.md`
