# Sub-slice 3s settlement completeness — dual-lens review record

Date: 2026-08-09. Artifact: `feat/r2f1b-3s-settlement` @ `d35f1075` → repaired
`6dfe7fb1` (base `c0d43429`). Opus senior-lead: REVISE — 1 WRONG/BLOCKER + 6
DEFER; verified the epilogue BY CONSTRUCTION (after the tracker exists there is
no bare `return` in the generator's scheduled region — stronger than the
handoff's own enumeration), mandate C a real four-field ownership predicate, V2
byte-identity with the injection seam compiling out entirely. Sol: REJECT — the
IDENTICAL blocker found independently + 3 SMELLs. Declared cap held: one round,
one repair.

**Pipeline record — the bridge run and the operator boundary.** The implement
container (terra/xhigh, clone `impl-22708-aff68jfg`) lost registry egress
(CONNECT 403) before its final round could compile; the tail shipped blind and
its handoff honestly reported the gates BLOCKED. Operator completions, each
mechanism-classified first and disclosed as review surface: the 13-site
test-initializer ripple, an invalid `AttemptId` fixture string, and a
post-admission map re-read in `preserve_checkout_v1`. **The review then refuted
the operator's own re-read comment** — the honest headline of this round.

## Adjudication highlights

- **Repaired (opus WRONG-1 / sol WRONG-1 — the same defect, independently):**
  the deletion-admission guard was released at the mint block's scope, one
  block BEFORE the map projection. A preservation writer admitted in the
  tombstone-to-projection window read the stale `Protected` entry for an
  already-removed checkout; the operator re-read's "exact, not best-effort"
  claim was false in the presence direction; and the contention test was green
  only by current-thread scheduling accident. Repair: the guard now outlives
  the mint and drops only after the map clear + `state.entry` reset. Red-first
  evidence with a probe-honesty note: the first form of the new regression
  (yield-loop pacing) was GREEN pre-fix — a NON-DISCRIMINATING probe, recorded,
  and replaced by a real-timer window that observed the writer completing
  inside the projection window (RED), then held at admission post-fix (GREEN,
  multi-thread). Reverting the guard hoist re-reds it. The
  `custody_lock.rs` order now names `deletion_admission`, the THIRD inverse
  nesting, and the prohibition (a file-cell holder never takes admission) —
  opus DEFER-2 / sol SMELL-2, the 2c2 declaration lesson enforced.
- **Repaired (docs):** the implementer handoff's design-note-2 sentence
  ("mint-first … the later writer sees no checkout") is corrected to name the
  repair that made it true; gates carry provenance labels (historical
  in-container BLOCKED vs current host-run); the exit-table's ±15 line drift
  and the two colliding "13"s are disambiguated (opus DEFER-7 / sol SMELL-3).
- **Ledgered, not repaired:** injection fidelity — the policy/encode/invariant
  family reds drive macro sites ADJACENT to the real error arms, not through
  them (opus DEFER-3 / sol SMELL-1); the named remedy is opus's by-construction
  tripwire (a source-level assertion that no `yield Err` follows the tracker)
  rather than more injection points; plus a terminal-error `Cancellation`
  mapping case and one per-field ownership negative (sol). The node-output exit
  has no dedicated red (opus DEFER-4 — rests on the by-construction argument).
  Mandate C's predicate silently converts an attempt-advanced (post-exchange)
  record's mint into a refusal — fail-closed and almost certainly intended, but
  unpinned (opus DEFER-5 → 3b2/slice 5). Order-1 of the contention pair is a
  dominance test, not a race (opus DEFER-6 — partially absorbed by the new
  multi-thread regression).

## Explicit verdicts carried from review

(a) Epilogue completeness + before-yield ordering — PASS, provable by
construction; the consumer-drop regression genuinely discriminates; exactly-once
asserted; the reason mapping shares the shipped `cancelled`-dominates predicate.
(b) Linearization — dominance verified in source; the check→CAS window closed
both directions; the guard scope defect above was the one residual, now closed.
(c) The operator re-read — REFUTED in the presence direction as shipped;
exact in both directions after R1 (the comment now states the mechanism).
(d) Mandate C — DISCHARGED, stronger than asked (four-field ownership predicate
+ effect-free foreign-record refusal).
(e) V2 byte-identity — HOLDS; the fault seam is `#[cfg(test)]`-only, zero
production code added by the injection surface.
(f) Sizing — 3s landed at roughly a third of its estimate: the FIRST sub-slice
in this program under estimate (a planning datapoint for the 3.2×-based model).

## Gates

Repair verified on host (darwin): diff-check/fmt/clippy clean; six-package
focused suite **2673 / 0 / 11 across 51** (implement head 2672 → +1, the new
projection-window regression). Fold-gate totals (workspace, release, hygiene,
coverage delta) recorded below this record's date line in the handoff ledger
per lane practice.

## Ledger

- **3a/3b (binding context):** the flight sub-slices inherit the linearized
  admission world; any new `deletion_admission` caller must respect the
  custody_lock prohibition (file-cell holders never take admission).
- **3b2/slice 5:** pin the attempt-advanced ownership shape (a post-exchange
  record's mint refusal is intended — make it a named test, opus DEFER-5).
- **Test-strength rows (with the 3d/3e-era test passes or slice 6):** the
  by-construction tripwire for the epilogue; injection-through-the-real-arm for
  the three defensive families; a terminal-error `Cancellation` mapping case;
  per-field ownership negatives; a real second contention order.
- **Environment row (bridge pipeline):** implement-container registry egress
  loss (CONNECT 403 mid-run) produced a blind tail for the second time
  pattern-adjacent (2c2's fix-loop chase was the first) — if a third run
  degrades, commission the egress-proxy stability investigation rather than
  absorbing at the operator boundary again.
- **Posture note:** the round's load-bearing find was AGAINST an operator
  completion, caught because the completions were declared review surface
  rather than folded silently — the disclosure discipline paid for itself. And
  the non-discriminating first probe (yield-loop) is recorded beside the
  discriminating one (real-timer): evidence admissibility applies to test
  design, not just debugging.
