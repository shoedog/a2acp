---
task-type: code-review
---
# R2f1b 3c2 Task A3 owner-authorized closure review

## Description

Perform the one declared hard-read-only closure review of the complete Task
A3 line: exact diff `3890fa6c..6114596d` in this checkout, where `3890fa6c`
is the accepted A2 candidate and `6114596d` is the current head. Review the
full diff, the complete changed files, and the bounded new public surface in
context. Do not edit, build, test, invoke another provider, or access the
network. This review round is capped at one; no repair loop is authorized.

The line contains four commits, all previously adjudicated by the operator
with owner authorization at each boundary:

1. `f6b6ccf6` — the A3 implementation (capture settlement and bounded crash
   recovery: replace/retire transactions over the A1 no-replace capture,
   immutable synced intents, distinct rollback/roll-forward recovery,
   recovery tickets, protective outcome lattice, plus the four A2 rider
   regressions). Its one independent Sol/xhigh review REJECTed with four
   proposed WRONGs.
2. `b1b55a21` — mechanical operator reformat only: the module-wide
   `#[rustfmt::skip]` was removed and `cargo fmt` run; focused suites 92/92
   before commit; zero semantic change intended.
3. `af6d874d` — the owner-authorized targeted repair: typed `Unsupported`
   preservation through a `Failure(unsupported, reason)` carrier with
   pre-mutation refusal where knowable and rollback-plus-typed-refusal for
   runtime `ENOTSUP` at capture; a SHA-256 staged-content commitment in the
   intent wire (Replace requires it, Retire must not carry it), verified on
   live publication and recovery success paths.
4. `6114596d` — a disclosed operator completion: `finish` re-verifies the
   staged commitment immediately before predecessor removal on replace paths
   (live and recovery); the same-cell mutex rider now proves queuing with
   ordering tokens instead of a bare timeout; new regressions pin
   recovery-time unsupported typing, missing-birthtime classification, and
   both wire commitment-presence negatives.

Operator adjudications you must independently judge (you are licensed to
contest each; the referenced owner rulings are the custody adjudication's
threat model — confirmed success covers cooperating participants that obey
the one operation lease inside an owner-private namespace; noncooperating
interference may produce protective outcomes, never success):

1. Round-1 W2 (verify-then-act name-based unlink/rename) was refuted as a
   blocker: POSIX offers no inode-conditional namespace mutation; every raced
   schedule lands `Retained` (held-descriptor zero-link proof, post-publication
   verification, protective recovery arms); the residual foreign-plant
   deletion inside bridge-reserved names is outside the success covenant.
2. Round-1 W3 (lock-name replacement yielding two authorized cells) was
   refuted: authority is the flock on the exact externally-bound object; a
   second authorized cell requires a second trusted binding, excluded by the
   design premise; the cited code is the unchanged A2-accepted surface.
3. Round-1 W1 and repair-round RW2 (same-length content mutation) were ruled
   a design-vocabulary question, answered by the owner with the SHA-256
   commitment; the unclosable post-verification window against a live
   same-UID writer is accepted as residual under the threat model. The
   completion narrows the final window to immediately before predecessor
   removal.
4. Repair-round RW1 (crash between an `ENOTSUP` capture refusal and its
   rollback recovers as `NoEffect` instead of `Unsupported`) was downgraded:
   recovery cannot durably know a refusal that was never durable, its
   `NoEffect` statement is provably true at emission, and the typed
   `Unsupported` resurfaces on the very next attempt with no state advanced.
   Judge whether any consumer decision within B-G scope could go wrong in
   that window.
5. Repair-round RW4 (nine `rustfmt::skip` attributes in changed files) was
   refuted as inherited: all nine predate A3 (accepted A1/A2 line-level table
   attributes); the A3 line introduces zero and the new module contains none.
6. Cap accounting was owner-regularized after disclosure: the implementation
   measured ~735 formatted production against its original 320 estimate; the
   repair measured 270 production / 492 total churn against its 150/350; the
   completion adds 166/5. The owner explicitly accepted these measured sizes
   (Path-2 decision, 2026-08-14); size is not an open question for this
   review, but silent scope beyond the named files would be.

Required judgments:

- Explicitly adjudicate the round-1 confirmed WRONG (typed `Unsupported`
  erasure) and the repair-round findings RW1/RW2 as FIXED, PARTIAL, OPEN, or
  ACCEPTED-RESIDUAL against the shipped line.
- Verify the recovery table end-to-end: every crash cut of replace and retire
  has a pinned, idempotent recovery outcome; only proved `Complete` projects
  success; `NoEffect` claims carry positive proof; malformed, duplicate,
  foreign, and over-cap residue is preserved and blocks; `Drop` performs no
  namespace cleanup; no `is_success`-style flattening exists.
- Verify the commitment: Replace intents require it, Retire intents reject
  it, live and recovery success paths verify it, and the pre-removal recheck
  runs on both replace `finish` call sites and neither retire site.
- Verify the A1/A2 accepted surfaces are behaviorally unchanged, the shared
  generation journal/worktree/`local_file`/reaper surfaces are untouched,
  `Cargo.lock` is unchanged, `ring` was already a dependency, no production
  caller/route/persistence/V3 arming exists, and the module is fully
  formatted with no `rustfmt::skip`.
- Judge the operator commits (`b1b55a21`, `6114596d`) specifically: the
  reformat must contain no semantic change, and the completion must match its
  declared scope.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `6114596d`, clean worktree, branch `implement/impl-50158-87hmxb7t`;
- cumulative A3 diff vs `3890fa6c`: `namespace_transaction.rs` +1,442 (new,
  formatted), `fs_custody.rs` +178/−25, `lib.rs` +2, handoff +18;
- red-first receipts: the post-digest mutation test failed at its `Retained`
  assertion on the pre-completion tree (retained log); the repair handoff
  records behavioral reds for both content-corruption cases and the errno
  mapping; the mutex rider passed 6/6 repeated runs;
- focused suites at head: 97 passed / 0 failed across the
  `namespace_transaction`/`custody_v2`/`fs_custody` selectors;
- operator host gates on exact `6114596d` all exit 0: `git diff --check`,
  formatter, locked all-target/all-feature workspace check and Clippy with
  `-D warnings`, full locked all-feature workspace test **4,019 passed / 0
  failed / 13 ignored across 90 harnesses** (ignored = the declared
  authenticated/live set), locked release build, `cargo deny check`, and
  repository hygiene 40 tracked / 8 configs;
- the two in-container verify reds on this lane (whole-bin `a2a-bridge`
  harness, flock-EBADF signature) are the recorded hermetic class; the exact
  head is host-green.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must name a
  constructible input/state, the incorrect result, realistic reachability,
  and a bounded fix.
- Explicitly adjudicate the inherited findings as listed above.
- Judge the six operator adjudications.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/namespace_transaction.rs`
- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-core/src/lib.rs`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the A1-A3 implementer statements)
- repository `AGENTS.md`
