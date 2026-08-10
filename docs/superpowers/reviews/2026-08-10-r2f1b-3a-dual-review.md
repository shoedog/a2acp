# Sub-slice 3a flight core — dual-lens review record

Date: 2026-08-10. Artifact: `feat/r2f1b-3a-flight-core` @ `b80e50a6` → repaired
`b7fc1177` (base `d2d9512f`). Opus senior-lead: REVISE — 2 WRONG/BLOCKER + 10
DEFER; verified the sync-join mechanism (zero async tokens, pure Condvar),
journal durability + hand-traced capacity arithmetic, the manual-clock
transfer, the shell-cannot-signal census, and the wire tripwire (+11 lines =
pure re-export; A2 goldens untouched). Sol: REJECT — 5 BLOCKERs + 3 SMELLs
(two identical to opus's; three escalations adjudicated below). Declared cap
held: one round, one repair.

**Pipeline record.** Three bridge-implement rounds (terra/xhigh): the initial
run hit its bound with real work outstanding (D1 compile / D2 recovery
publication / D3 CAS bypass — no environment failure; `implement --resume`
correctly refused the terminal-phase run, so a bounded continuation closed
D1–D3); the dual lens then drove the declared repair round (RA–RF), whose
container overlapped the 2026-08-09 20:15–21:10 MDT house power/network
outage — the CONNECT 403 in its handoff was the OUTAGE, not proxy degradation
(run-log verified: zero 403 events; the real proxy-degradation count stays at
2). Operator completions, disclosed as review surface each time: two
mechanical test-code compile fixes on the repair tail (a missing `AtomicBool`
import; one moved-value clone), and the typed-spec gate rejected the
orchestrator's own first continuation brief (missing Acceptance Criteria) —
the E7 gate working on the operator's input.

## Adjudication highlights

- **Repaired RA (opus W1 + sol W1, composed):** the recovery route was an
  unfenced `pub` settlement path — a live flight could settle after a recovery
  already settled the same id (two contradictory `Settled` rows, double
  publication), two concurrent recoveries double-published, and a second
  registry over the same root could "recover" a live flight. Fix: a terminal
  compare-and-set at the `ResourceFlightJournal` port (terminal append returns
  `AlreadySettled` under the append lock; publish only on the winning append)
  with `settle_locked` ADOPTING an already-settled terminal — all variants
  converge to one terminal, one publication set.
  `concurrent_recovery_has_one_terminal_cas_winner` + the live-settle-adopt
  regression pin it. Sol's lease-proven-liveness ask = slice-5 ledger; the
  ONE-REGISTRY-PER-ATTEMPT invariant is 3b1's documented wiring obligation.
- **Repaired RB (opus W2 = sol W5):** `join_blocking`'s only `notify_all` sat
  behind a fallible append — journal I/O failure or a dispatcher panic
  stranded a bare-thread joiner (the exact B13 consumer) forever. Fix: a typed
  in-memory terminal refusal carrying the journal error + `notify_all` on the
  failure path; the regression is watchdog-bounded so a regression fails
  rather than hangs.
- **Repaired RC (opus W3 / sol W4, the in-mandate halves):** the reservation
  file now publishes via write-temp→fsync→no-replace-rename (a kill between
  create and write no longer bricks the generation key), and journal reads
  tolerate exactly one unterminated trailing record (crash/reopen now settles
  `Unknown` as mandated instead of failing `Decode`;
  `torn_journal_tail_is_truncated_and_recovery_settles_unknown`).
  Cross-handle/process journal serialization = slice-5 ledger.
- **Repaired RD (opus S11 = sol S2):** durable recovery completes under the
  registry mutex, publication runs AFTER release
  (`recovery_publication_runs_after_releasing_registry_mutex` re-enters
  `reserve` under a watchdog); the module lock declaration now covers the
  registry mutex.
- **Repaired RE (seven small):** `affected_owner_count` on the aggregation
  (slice 5's `CollateralResultV1` requires it); the row-10
  `GuardTransferred`-before-`Settled` sequence assert; `compile_fail,E0624`;
  a failed foreign transfer returns its guard (and an adopted recovery
  terminal reports `Unknown` with the exact guard rather than falsely
  `Transferred` — the implementer widened S9 correctly); the registry
  fast-path attempt check; `cfg(test)` doubles + opaque dispatch admission;
  discovery covered in `Signaling` and post-`Settled`.
- **Adjudicated to slice 5, DISCLOSED not silent (opus W4 = sol W2; sol W3):**
  publication is at-most-once ACROSS A CRASH (the `Settled` row is durable
  before owner publication; a crash in that window loses deliveries — the
  durable per-owner outbox with acknowledged publication is slice-5 recovery
  machinery, and the handoff now carries the window explicitly). The
  guard-transfer prepare/accept redesign (sol W3: a joiner can observe the
  terminal while the exact guard is still in the transfer call; a crash there
  drops it) is the same slice-5 acknowledged-sink family — the in-slice half
  (journaled ordering + the sequence assert) is pinned; severity adjudicated
  down from sol's "critical" on reachability (no production consumer until
  3b1, and process death destroys in-memory guards regardless — the durable
  story is the journal row, which is correctly ordered).

## Explicit verdicts carried from review

(a) Sync-joinability — mechanism PASS (grep-verified zero async/tokio/block_on
tokens; Condvar join; no runtime dependence); bounding FIXED by RB.
(b) Journal durability/capacity/crash-reopen — PASS/PASS/now-unqualified
(RC closed the torn windows); capacity arithmetic hand-traced exact; §9
sequencing present in the mechanism (`outstanding()` contiguity).
(c) D2 exactly-one-result — PASS in-process (structural after RA); the
across-crash at-most-once window is the disclosed slice-5 remainder.
(d) D3 unforgeability — construction PASS (one path, private ctor, censused);
the settlement-side bypass was the real gap and RA fenced it.
(e) Manual-clock transfer — PASS (the ONE injected clock; zero production
timers; exact guard by value).
(f) Shell-cannot-signal — PASS (three methods, no libc/Command/handle;
identity injected, not probed). `records()` exposes durable pids — 3b1 must
never derive a signal path from them (carried note).
(g) Wire tripwire — CLEAN (re-exports only; goldens pin the same bytes).

## Gates

Post-repair on host (darwin, worktree `s3a`): diff-check/fmt/clippy clean;
six-package suite **2705 / 0 / 11 across 51** (pre-repair 2693 → +12 repair
regressions). Container reds across the three rounds were the known
environmental families or outage-induced; every terminal adjudication was a
host run. Fold-gate totals + coverage delta in the addendum below.

## Ledger

- **3b1 (binding):** ONE registry per attempt (RA's adjudication basis);
  never build a signal path off `records()`; the full
  `two_nodes_one_generation_signal_once_and_share_result` (real signal) and
  the platform start-identity probes land there; join via the typed refusal
  path (RB) from `terminate_blocking`'s bare thread.
- **Slice 5 (binding, disclosed):** durable per-owner publication outbox with
  acknowledged delivery (closes the at-most-once-across-crash window);
  lease-proven-dead liveness before recovery; guard-transfer
  prepare/accept/settle with an acknowledged recovery sink; cross-handle/
  process journal serialization.
- **Test-strength (with later passes):** sol SMELL-3's enumeration minus what
  RA–RE landed; opus's cap-boundary negative (capacity "exact" shown as
  sufficient, not necessary).
- **Posture notes:** the initial run's bound-reached was REAL WORK remaining
  (first time in this program a bound hit without an environment cause) — the
  3.2×-informed estimate held (~3,100 landed vs ~2,500 estimate, inside the
  1.5× STOP); and the two lenses' complementary censuses (opus: construction
  clean, settlement bypassed; sol: the cross-registry variant) again justified
  dual-lens on a non-destructive sub-slice — 3a's semantics are inherited by
  five destructive ones.

## Fold addendum (post-push)

Fold gates at `4597feb9`: all seven stages clean — **workspace 3866 / 0 / 12
across 90** (3834 → +32), release + hygiene OK. Coverage vs the 3s seed
(lib scope, darwin): bridge-core **87.69%** (+0.21), bridge-workflow **84.90%**
(unchanged — 3a lands entirely in bridge-core). Fold-ritual reaps: s3a
worktree target + the three 3a implement-clone targets; receipts in the
session scratchpad.
