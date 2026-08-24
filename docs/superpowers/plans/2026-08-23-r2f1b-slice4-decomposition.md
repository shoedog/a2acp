# R2f1b slice 4 — Scheduler activation: decomposition

**Base:** `origin/main` = `3dadec91`. Slices 1–3 are complete.

**Provenance.** Folded by the operator from two independently authored decompositions given byte-identical
input — `gpt-5.6-sol` (effort xhigh, 933 s, 10 sub-slices, 6,240 projected) and `opencode-go/ox-alpha-free`
(206 s, 6 sub-slices, ≤2,110). Where they disagreed, the operator verified against the frozen scope document
and the tree rather than choosing by preference. Both authors' reasoning is credited inline.

## 0. What this slice is

Slice 4 is the first R2f1b slice that changes production behaviour. It is where `AutomaticR2f1b` stops being
inert and deadlines can actually fire.

Per the frozen scope document §7 item 4: progress notifications, queue/control/fixed-grace/absolute clocks,
preservation-first cancellation, bounded cleanup transfer, fixed-grace refusal lift, #22 closure.

## 1. Two corrections the fold makes to the source plans

### 1.1 The custody-admission layer is NOT slice 4 scope — dropped

`gpt-5.6-sol` scoped a 950-cap sub-slice containing a frozen provider-attempt checkout matrix, custody-plan
derivation with dedup by checkout identity, `FrozenR2f1bContractV1` minting inside the admission boundary,
`WorkflowSnapshotV3::workload_identity()` binding, and **a fixed admission cap of 64 unique custody plans**.

The operator verified all of it:

| Claim | Measured on `3dadec91` |
|---|---|
| `FrozenR2f1bContractV1` needs building | **Already exists — 55 references** |
| `workload_identity` binding needs building | **Already exists — 23 references** |
| A 64-plan admission cap is required | **Zero references in the frozen scope document** |

The contract and identity machinery landed in slices 1–3. The 64-cap is not in the frozen scope at all — it
is an invented bound, and a resource-admission bound is owner authority, not an author's choice. **Dropped.**
This is the single largest reason the two projections differed by 3×: sol over-scoped; ox did not
under-scope.

### 1.2 `AutomaticR2f1b` becomes constructible EARLY, not late

`opencode-go/ox-alpha-free` placed constructibility second; `gpt-5.6-sol` placed it ninth. **ox is right**,
and its argument stands on its own:

> The variant becomes real in a sub-slice whose entire behavioural delta is *refusals and identity bytes* —
> zero scheduling change. Reverting it alone restores the pre-change tree exactly; nothing downstream can
> misbehave because nothing downstream reads the value yet. Deferring constructibility to the last sub-slice
> instead would bundle admission, fingerprinting, and watchdog gating into the same diff as live clocks and
> cancellation — precisely the big-bang shape this lane forbids.

With §1.1 applied the argument is stronger still: because the contract type already exists, making it
*admissible* is genuinely a small validation-and-fingerprint change.

The cost is honest and accepted: the type is constructible while inert for eight subsequent sub-slices, and
every one of them must keep refusing. §5 makes that a standing gate rather than a hope.

## 2. Frozen — binding on every sub-slice, no exceptions

- The eight **observable** bounds in `liveness_profile_v1` are unchanged: queue wait 1,800,000 ms; control
  observable 31,000; no-progress snapshot 1,800,000; work cutoff 7,200,000; cancel observable 6,000; cleanup
  tail 60,000; reporting tail 10,000; terminal envelope 7,270,000.
- D11's **internal** action timers — 30 s control, 5 s cancellation grace — are added as R2f1b constants,
  with the remaining second reserved for scheduling/fencing/publication. **Lengthening the internal timers to
  match the observable bounds is forbidden by D11.**
- One clock. Reuse `bridge_core::attempt_activity::MonotonicClock` / `SystemMonotonicClock`. One
  `Arc<dyn MonotonicClock>` per attempt feeds recorder, telemetry, scheduler, cleanup and reporting. **Do not
  invent a parallel clock type.** Wall timestamps identify records only; monotonic offsets are audit data,
  never restartable wall deadlines. Resume starts a new monotonic epoch under unchanged frozen policy.
- **Silence never cancels.**
- Early automatic cancellation is a **closed list**: a retained child exited while its sole producer result
  is pending; a named container generation proved absent after spawn settlement; all producer/final routes
  irreversibly closed with no terminal result possible. Explicitly not proof: unknown child state, no output,
  elapsed silence, file mtime, process age, provider slowness.
- Fixed grace is **one-shot and non-renewable**, records its separately named policy trigger, and never
  rewrites a sibling's recorded node deadline.
- No new resource-admission cap. No change to `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`.

## 3. Sizing policy

Counted lines are **added nonblank physical Rust lines after `cargo fmt`**. Documentation, retained command
output and frozen mutation patches are excluded. Deleted lines do not offset additions.

Adopted verbatim from `gpt-5.6-sol`, because it is the discipline T3b lacked:

> The aggregate caps are a **stop boundary, not a target or contingency budget**. Exceeding any individual
> cap requires splitting that sub-slice before review. **Unused capacity in one sub-slice cannot be
> transferred to another.**

| # | Sub-slice | Projection | Cap |
|---|---|---:|---:|
| 4A | Shared attempt clock and D11 constants | 300 | 450 |
| 4B | Constructible, fully refused | 250 | 350 |
| 4C | Preservation-first cancellation and bounded cleanup transfer | 350 | 500 |
| 4D | Scheduler arbitration kernel | 300 | 450 |
| 4E | Constructive impossibility proof adapters | 280 | 400 |
| 4F | Durable fixed-grace timer (gated) | 200 | 300 |
| 4G | Progress epochs and no-progress warnings | 250 | 350 |
| 4H | Eight-arm executor multiplexer | 350 | 500 |
| 4I | Issue #22 terminalization closure | 280 | 400 |
| 4J | Production arming commit | 50 | 80 |
| | **Total** | **2,610** | **3,780** |

For calibration: T3b was ~3,880 in caps across six increments.

## 4. Sub-slices, in commit order

Each is one branch and one non-amended commit on the previous. The order is dependency-aware and
risk-front-loaded — every scheduler input gets an isolated test seam before the loop integrates it.

**4A — Shared attempt clock and D11 constants.** One `Arc<dyn MonotonicClock>` threaded to every consumer;
the 30 s control and 5 s cancellation-grace constants added; pure schedule math with a fake clock. No timer
arms. *Proves:* one clock identity, and that internal timers are shorter than their observable bounds.

**4B — Constructible, fully refused.** First construction site of `DeadlineActivationV2::AutomaticR2f1b`:
admitted in frozen-controls validation under `PolicyActivationV1::Production` (today hardcoded to
`ManualOnlyR2f1a`), included in the workload fingerprint so armed and manual runs never pool into one
calibration population, and an ACP admission check refusing legacy `[agents.watchdog]` before any registry,
session or provider effect. **Every action consumer still refuses**: fixed grace stays inactive, no timers
arm, the event loop is untouched. *Proves:* construction without consumption.

**4C — Preservation-first cancellation and bounded cleanup transfer.** Preservation and exact ownership
settled before any cancellation path exists.

**4D — Scheduler arbitration kernel.** The eight-arm priority order, with its tie rules: completion ready
**at** the cutoff wins for that node and unfinished nodes are then cancelled; warning loses to both
completion and cutoff. Per `gpt-5.6-sol`, and adopted: *the priority order is represented once in executable
code and once in a table-driven test — there must not be separate production and test priority
implementations.*

**4E — Constructive impossibility proof adapters.** The closed list, and its negatives. Reviewed before
lower-impact warning behaviour because a false positive here cancels real work.

**4F — Durable fixed-grace timer (gated).** One-shot, non-renewable, separately named trigger. Built and
tested; not yet reachable from production.

**4G — Progress epochs and no-progress warnings.** `ordinal = floor((now - last_meaningful_progress) / 30m)`,
each positive ordinal emitting once per progress epoch; activity without meaningful progress updates only the
activity clock; progress resets the epoch. *Proves:* silence never cancels.

**4H — Eight-arm executor multiplexer.** Replace the bare `let Some(first) = inflight.next().await` in
`crates/bridge-workflow/src/executor.rs` with the `biased` select. `FuturesUnordered` is kept. Integrated only
now, because 4A–4G gave every arm an isolated seam.

**4I — Issue #22 terminalization closure.** A nonterminating sibling no longer blocks terminalization.
Separated from 4H deliberately: `ox-alpha` merged these two and named the merged unit its own dominant
uncertainty — "the most likely to be exceeded" — which is exactly where a finer seam earns its keep.

**4J — Production arming commit.** Flip readiness. Minimal, last, independently revertable. Its revert
disarms the subsystem completely, exactly as T3b's readiness flip did.

## 5. Standing gates

- **RED-first.** Every sub-slice ships at least one test that fails on the pre-change tree. A test that
  cannot fail pre-change does not discharge this — verify, do not assume.
- **The refusal gate.** 4B through 4I must each re-assert that no production caller can construct an
  automatic attempt. This is the accepted cost of early constructibility; it is checked every time, not once.
- **Frozen control per sub-slice**, mutating **production** and not a test fixture. A control reddening more
  than one test is acceptable and usually stronger — it means the obligation is enforced in more than one
  place. Report the **actual** reddened population from a **full-suite** run; a population derived from a
  filtered run is not a population.
- **Controls must survive `clippy -D warnings`**, not merely `cargo test`. A control that fails on dead_code
  before reaching its red tests proves nothing.
- Handoffs never record their own head or tree sha; that binding is the operator's evidence commit.

## 6. Residuals carried, not solved

- The type is constructible-but-inert across 4B–4I. §5's refusal gate is the mitigation, not a proof.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT` is untouched; whether a *separate* retained-resource admission bound is
  needed is a real question, but it is **owner authority and outside slice 4**. It is recorded here because
  `gpt-5.6-sol` raised it, not because this plan adopts it.
- The scope document's line numbers are stale — `liveness_profile_v1` moved 107 → 128, the #22 site 4619 →
  5233. Cite by symbol.
