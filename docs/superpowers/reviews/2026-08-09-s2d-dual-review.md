# Slice 2d claim-exchange mechanism — dual-lens review record

Date: 2026-08-09. Artifact: `feat/r2f1b-2d-claim-exchange` @ `85c0b33d` → repaired
`f0d32965` (base `c13ff663`). Opus senior-lead: REVISE — 1 WRONG/BLOCKER + 9
SMELL/DEFER + 2 mandate gaps; verified sweep/gate/mint protection inheritance,
validate-before-effect completeness, the normative frozen-edge reading, crash-window
ordering, and token unforgeability all SOUND in source. Sol: REJECT — 3 BLOCKERs +
3 SMELLs. Declared cap held: one round, one repair, no second review. Implemented
and repaired through the bridge (gpt-5.6-terra/xhigh — standing owner directive).

## Adjudication highlights

- **Repaired (opus F1 / sol-1 — found INDEPENDENTLY by both lenses; orchestrator
  verified all three mechanism legs in source):** the exchange never consulted
  predecessor liveness. `run_id()` is the attempt id, `validate_successor`
  REQUIRES the successor's to differ, and the only lease call keyed on the
  successor — so the successor lease could never contend with a live
  predecessor's, `Exchanged` was reachable while the predecessor ran, and the
  durable `RecoveredLive` then refused the live predecessor's own
  `preserve_after_cancel` (no legal preservation edge from `recovered_live`).
  §5.8 step 3 was unimplemented; §5.7 row 6's lease half untested (the row-6 test
  created no lease at all). Repair RA': the predecessor's recovery lease is
  acquired after validation and BEFORE the cells (held → byte-identical refusal —
  a held flock is the alive signal, a free flock the crash signal), retained
  across publication, and released only after the successor lease is held — that
  ordering IS the transfer. Row 6 now proves the lease half (flock-held while the
  token lives; reacquirable after drop).
- **Repaired (sol-2; opus F7 concurring at SMELL):** the exchange accepted a
  canonically-valid `LiveProtected` record under a WRONG root (binding validation
  discarded the matched checkout's `canonical_worktree_root`; the record read
  accepted any record matching caller-supplied identities) — the token could
  authorize continuation against a never-validated directory. Repair RB': the
  matched frozen checkout is returned from binding validation; root equality,
  target-parent equality, and retained/record path binding are enforced before
  any effect; wrong-root / wrong-record-worktree / swapped-retained negatives
  prove zero effects.
- **Repaired (sol-3 — the 2c1 `PreservationPrepared` class):** `LeaseUnavailable`
  (or an ambiguous-but-landed replace) stranded a durable `RecoveredLive` nothing
  could re-enter — the retry refused at the from-state check; permanently
  unresumable in exactly the crash environment the mechanism exists for. Repair
  RC': an idempotent re-entry arm on an EXACT match (successor attempt, digest,
  custody identity, frozen target, reverified identities) acquires the missing
  successor lease and REWRITES NOTHING (no table edge exists and none was added —
  record byte-identity pinned); any mismatch refuses; RA's predecessor-lease rule
  applies to re-entry identically.
- **RD' small items:** `#[must_use]` on `ClaimExchangeOutcomeV1` (both lenses);
  blocking-flock doc on the API; the `predecessor_claim_digest` field documented
  as holding the predecessor SNAPSHOT digest (no claim exists on the normative
  edge — opus F2; slice 5's §5.8-step-4 consumer must not expect a claim digest);
  sol S-1's pristine-root no-lock-creation negative; the stale 2c2-handoff test
  name (opus F10); handoff corrections (liveness phase added to design note 2,
  the vacuous "retiring claim content" phrase deleted, the P3 mutation claim
  corrected — it discriminates via the state assertion, not any sweep assertion).
- **Internal-review objection adjudicated against source (operator):** the bridge
  fix loop's third-round REJECT claimed two RB' tests failed the
  "pristine-root" zero-effect criterion. Direct read: those cases structurally
  REQUIRE a pre-existing record (a wrong-record-worktree case cannot exist on a
  pristine root); the tests snapshot the `.custody-locks` entry list and assert
  it UNCHANGED — the correct zero-effect instrument there, since locks are never
  unlinked by design. The genuinely-pristine negative exists separately. The
  criterion's wording over-generalized; the tests are right. Same
  acceptance-literalism class as A4's sol REJECT.

## Explicit verdicts carried from review

Opus verified SOUND in source: (a) `RecoveredLive` protection inheritance — real
boot-sweep and run-end arms non-destructive for every V3 record, gate refusal via
the state-agnostic presence arm, and the 2c2 mint genuinely driven to refusal by
a real `RecoveredLive` record ("the strongest new test in the slice");
(b) validate-before-effect complete (both snapshots, `validate_successor`, both
bindings, plan/node, digest, frozen target — all strictly before `enter`);
(c) exactly the normative `LiveProtected → RecoveredLive` edge, double-guarded;
`Preserved` refuses byte-identically (the §2.2 diagram edge stays non-normative);
claim-null destroys nothing (the from-state forbids a claim); (d) publication
strictly before lease, cells dropped first — NO new custody→lease nesting (the
2c2 declaration lesson satisfied by avoidance); (e) `ClaimExchangeReadyV1`
unforgeable (private fields, no constructor — `DeletionCapabilityV1` parity) and
its `Drop` releases the lease cleanly. Evidence-quality note (opus): of the five
recorded mutation checks, four traced sound; the P3 mutation does not witness the
sweep property it was offered for (the property is true — pinned by 2a's unit
test — but this slice's assertions do not discriminate it). Red-first
observations were disclosed as not retained; the repair round's three mutations
were run and recorded.

## Gates

Repair verified on host (darwin, worktree under `~/code`): `git diff --check`,
fmt, clippy `-D warnings` clean; six-package focused suite **2665 / 0 / 11 across
51 binaries** (implement head 2658 → +7). The bridge's hermetic container verify
red (`fs_custody` errno-identity: `ENOTDIR` vs `ELOOP` — refusal held, errno
differed on the container mount) was adjudicated environmental with host
controls (bridge-core lib 481/0) and pre-named in the repair spec as
not-to-be-chased; the fix loop honored it. Fold-gate totals live in the handoff
ledger beside this record's date line.

**Slice-3 gate (brief §3 / §7 item 2): DISCHARGED.** §5.7 rows 1–6 and 12 all
green BY NAME on host: rows 1/4 (`backend.rs`), 2/3/5/12 (`custody_writer.rs`),
6 + the exchange half of 12 (`tests/r2f1b_claim_exchange.rs`) — row 6 now
covering BOTH protections including the real lease. Rows 7–11 are slice 3's.

## Ledger

- **Slice 5 (binding, prerequisite):** `RecoveredLive` has NO outgoing edges in
  the frozen table (2a deliberately deferred them; 2d's non-goals forbade
  adding) — every successfully exchanged checkout is protective-but-terminal
  until the table is amended; the slice that wires production resume must land
  the outgoing edges FIRST (opus F5). And the §5.8-step-4 consumer must treat
  `predecessor_claim_digest` as a SNAPSHOT digest (opus F2).
- **Slice-5 activation gate (sol S-2):** retained source/root/common-dir
  identities exist only in the in-memory `ProtectedCheckoutV1`; after a real
  crash no persisted source supplies them, and re-observing at recovery would
  defeat the substitution defence. Durable identity evidence is an activation
  precondition for production resume, not an assumed input.
- **Owner question (opus mandate gap 1):** brief §7 assigns the §6 "Candidate
  settlement" row wholly to 2d, but §3's 2d steps omit it;
  `unused_candidate_settles_only_after_exact_absence` does not exist and
  `UnusedSettled` remains producerless (2b2's recovery-side ruling). Owner
  disposition needed: re-assign to the recovery-side owner (slice 3/5) or
  commission it separately. Recorded in the s2d handoff's owner questions too.
- **Evidence notes (no code change):** the P1 counting-double assertions are
  entailed by the refusal match (the record-byte and lease-dir assertions are
  the real witnesses — opus F4); the P3 sweep assertions do not discriminate
  (opus F3); post-`enter` refusal arms beyond the repair's new negatives remain
  unexercised (opus F8 — partially closed by RB'/RC' tests).
- **Posture notes:** the same load-bearing defect surfaced independently under
  both lenses reading against §5.8 — the spec-vs-code read remains the
  highest-yield review posture on this program. The bridge fix-loop's REJECT was
  acceptance-literalism a direct source read dissolved; the operator boundary
  (inspect, verify the objection, then decide) cut both ways this round —
  stripping out-of-scope surgery in 2c2, overruling a wrong objection in 2d.
