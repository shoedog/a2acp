# Slice 2c2 deletion capability — dual-lens review record

Date: 2026-08-09. Artifact: `feat/r2f1b-2c2-deletion` @ `66f8ab0c` → repaired
`e26a87e3` (base `23909d5c`). Opus senior-lead: REVISE — 1 WRONG repair-now + 1
WRONG DEFER + 4 SMELL; verified the gate-bypass substitution, mint unforgeability,
monotonicity + epoch, health predicate, V2 byte-identity (independently recomputed
the removed-line set and diffed `remove_and_verify` against the old `remove` body:
byte-identical), and both binding ledger items against source. Sol: REJECT — 3
BLOCKERs + 2 SMELLs. Declared cap held: one round, one repair, no second review.

**Pipeline note.** The repair round was implemented through the bridge's own
containerized `implement` flow (gpt-5.6-terra/xhigh — owner directive: route
implementation through the bridge; dogfood + the pre-2026-08-11 codex usage
window). The flow's verify caught a mid-flight compile error and fed it back; its
internal review REJECTed the fix loop's out-of-scope `compatibility.rs` changes,
and the operator concurred — those changes were stripped at the hand-off boundary
(cherry-pick minus the file). Working config: `examples/a2a-bridge.2c2-repair-impl.toml`;
`--lang rust` is required (four language markers make auto-detection a hard error).

## Adjudication highlights

- **Repaired (sol-1, WRONG):** the capability branch ignored the inner-teardown
  result — a `GloballyHealthy` settlement whose final `release_session_checked`
  FAILED still minted, removed the checkout, and reported `Removed` beside
  `result: Err`. Control: the V2 removal block has the same
  proceed-despite-`first_error` shape, but V2 carries no preservation contract and
  §5.1 makes a cleanup-ambiguous outcome a preserve-trigger for V3. Narrow fix
  adjudicated: the mint requires this flight's inner teardown to have succeeded;
  on failure the flow falls to the unchanged gate (typed `Retained`, record stays
  `live_protected`, recovery owns). Per-checkout independence RULED: an earlier
  sibling's clean capability removal stands when a later sibling's release fails —
  its removal was verified under a genuinely healthy computed outcome. Sol's
  stronger two-phase shape (tear down everything, recompute health, then mint) is
  a §5.1-interpretation question, LEDGERED for the owner/slice 5.
- **Repaired (sol-3, WRONG):** `RemovedRecordAmbiguous` was collapsed to an
  unqualified `Removed` at the report layer (a log line carried the only trace),
  while `CheckoutSettlementV1::Removed`'s contract says "the record is its
  tombstone". Now typed end-to-end: `CheckoutCleanupDispositionV1` and
  `CheckoutSettlementV1` both carry `RemovedRecordAmbiguous(detail)`, with its own
  teardown code (`worktree.teardown.removed_record_ambiguous`); map-clearing
  unchanged (the checkout is verifiably gone — keeping the entry would wedge the
  session id). Red-first via the `fs_custody` parent-sync seam armed after the
  authorizing replace.
- **Repaired (opus W-1, WRONG):** the settlement's `NotHealthy` arm mapped a
  settled `PreservationUnknown` to `Retained` — the exact mislabel class opus W3
  made binding on this slice, reopened in the channel this slice added (the flight
  report and refusal arm both already said `Preserved`). Now `Preserved`.
- **Deferred WRONG, ledgered with owner (opus W-2 / sol-2 — both lenses found it
  independently):** the post-loop settlement runs only on `execute`'s ordinary
  fall-through tail; ~20 `yield Err; return` exits (harvest-audit, policy
  finalization/encoding, invariant failures — several reachable AFTER checkouts
  were recorded, and the policy-gated ones are precisely the V3 population) bypass
  it, so those outcomes neither preserve nor settle. Direction protective
  (`LiveProtected` is sweep-ineligible; recovery-owned), pre-2c2 behavior on those
  paths identical, V3 production-unreachable through slice 2. Sol's BLOCKER
  resolved by correcting the handoff's overclaims (P3 row, §4.7) and this ledger
  row rather than restructuring `execute` in a repair round. **Owner: the slice
  that restructures `execute`'s exits or activates V3 (slice 3/5).**
- **Doc repairs in-round:** the `ports.rs` composition claim was FALSE as written
  (opus SMELL-1 census: `ContainerRwBackend` is a second production `AgentBackend`
  decorator, wired unwrapped, forwarding neither custody method — the invariant
  actually holds by spawn-factory construction, its inner is always an
  `AcpBackend`); the `custody_lock.rs` declaration now names the capability
  branch as the SECOND deliberate inverse lock nesting with its no-cycle basis
  (sol S-2); the epoch claim restated precisely — the join→mint window is closed
  by the `LiveProtected` from-state CAS under both custody cells, the epoch guard
  is belt-and-braces and its check is NOT linearized with the mint (sol S-1;
  theoretical-only while no concurrent post-loop preservation caller exists —
  that caller is the ledger trigger).

## Explicit verdicts carried from review

Opus verified SOUND against source: (a) the gate-bypass substitution — the gate
returns `Discriminated` unconditionally for `Protected` entries, so it carries no
evidence the capability path lacks; both custody cells held across
mint→remove→tombstone is strictly stronger than the gate's probe→removal window;
no-deadlock verified against the 2b2 lock order including sweep-side users;
(b) mint unforgeability — one construction expression workspace-wide, private
fields, no Clone/Copy, consumed by value, failed revalidation consumes too;
(c) monotonicity both orders — `max(cell, record)` can only raise; M2's mutation
reproduced opus W3's literal mislabel pre-fix; (d) no-re-mint = the from-state
check (correction 3 is the accurate phrasing); (e) V2 byte-identity — the 10
removed `backend.rs` lines are exactly as claimed, `remove_and_verify` is a
byte-identical move, the 13 legacy tests untouched; (f) the frozen table:
`LEGAL_CUSTODY_TRANSITIONS_V1` byte-identical, `LiveProtected → DeleteAuthorized
→ Removed` pre-existed (2a), no new edges (pinned by test).

## Gates

Repair verified on host (darwin, worktree under `~/code`): `git diff --check`,
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
warnings` all clean; six-package focused suite **2652 passed / 0 failed / 11
ignored across 50 binaries** (pre-repair 2647 → +5, the RA–RC regressions).
Mutation checks RA/RB/RC applied, observed red, reverted (one RB probe was
syntactically invalid and recorded INADMISSIBLE; the anchored retry is the
evidence). Fold-gate totals (workspace suite, release build, repo-hygiene) are in
the fold commit's record below this file's date line in the handoff ledger.

**Flake attribution (evidence discipline):** one host run of the whole-bin
parallel suite failed `authority_mutation_lock_release_failure_is_loud_not_silent`
(flock LOCK_UN EBADF → fresh acquire LockBusy — a concurrently forked child
inheriting the lock fd until exec). The bridge's hermetic verify failed its
sibling `owner_admission_lock_release_failure_is_loud_not_silent` plus two
`staged_candidate*` exec tests in-container. Controls: targeted host runs 1/1 and
4/4 green; repair-commit re-run green (1081/0); base control green (1081/0); the
failing files are untouched by the diff. Verdict: the pre-existing #9/F-3
fork-inheritance flake family — the two `*_lock_release_failure_is_loud_not_silent`
tests are NEWLY OBSERVED members of that ledgered population; the container
failures are additionally hermetic-environment candidates for the impl config's
skip list. Not attributable to 2c2.

## Ledger

- **Slice 3/5 (binding):** the error-exit settlement population (opus W-2/sol-2
  above) — every `yield Err` exit after checkout recording must settle
  `NotHealthy` exactly once; owner = whichever slice restructures `execute`'s
  exits or first activates V3.
- **Owner/slice 5:** the two-phase settlement question (tear down all checkouts,
  recompute health including teardown results, then mint) vs the shipped
  per-checkout independence — a §5.1 reading the owner should ratify.
- **Recovery slice:** a proof-of-removal token (`remove_v2` returning evidence
  `record_removed` consumes — opus SMELL-2; ordering is caller-enforced today,
  verified correct); transient `remove_v2` failure is permanently terminal
  (`DeleteAuthorized` with no live retry — recovery-owned by design, opus
  SMELL-3).
- **Inherited, known:** `authorize_deletion` checks identity but not record
  ownership (opus SMELL-4 — same shape 2c1 shipped; no constructible failure
  through slice 2).
- **Trigger-gated:** epoch/mint linearization + a preserve-vs-mint barrier test
  (trigger: any new concurrent preservation/settlement caller after the post-loop
  pass); a deterministic writer-vs-settlement contention test (sol S-2 half).
- **Impl-config candidates:** add the hermetic-container exclusions for
  `owner_admission_lock_release_failure_is_loud_not_silent` /
  `staged_candidate_exec_is_bound_to_the_verified_file_object` /
  `staged_candidate_nonzero_exit_retains_process_status` to the containerized
  verify's skip list (same family as the three `process::` skips).
- **Standing (from the slice itself):** §4.3 preflight/V3 frozen-target collision
  (slice 5 finds it armed otherwise); preserved preflight checkouts accumulate on
  healthy V3 runs (accepted 2c1 residual); off-unix the automatic deletion path
  is entirely unavailable (mint and revalidation both need `dev`/`ino`) —
  protective, but checkouts accumulate; R2f2 owns the disposition of `Preserved`
  / `PreservationUnknown` / stranded `DeleteAuthorized` records.
- **Posture note:** both lenses independently found the same executor defect
  (W-2/sol-2) by reading `execute`'s control flow the diff did not touch — the
  2c1 lesson (transitive behavior of the funnel is review surface) held its
  value. The implement-flow dogfood surfaced real pipeline lessons: the fix loop
  will chase environment-red tests into out-of-scope production surgery — the
  operator boundary (inspect the diff, strip, control-test) is load-bearing.
