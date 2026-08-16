# R2f1b 3d dispatch brief — DRAFT (2026-08-15)

Status: DRAFT. Dispatch blocked on (1) the ledger-discharge slice landing,
(2) owner glance at the B21 resolution below. Source of authority:
`docs/superpowers/plans/2026-08-09-r2f1b-slice3-brief.md` §3d (landed
`c0d43429`); anchors re-measured on live main `6ad88565` (the slice-3 brief's
anchors predate 3a–3c2 and have shifted).

## B21 resolution (required before implementation; R3-7)

**Question.** `PreparationFlightStateV1` landed (A2) with
`Open {} / BarrierSynced {} / Transferred { reason } / Failed { cause }` and no
success-settlement state. Is `Transferred` the success terminal, or does the
wire type need amendment?

**Resolution: amend the wire — add `Settled {}` as the success-settlement
terminal.** Mechanism reasoning:

- §2.5 of the focused-boundary design fixes `Transferred`'s meaning: bound
  expiry before the prepared barrier "transfers the exact preparation guard to
  the recovery flight rather than dropping it." `Transferred` is the
  finite-ownership escape to RECOVERY, and its `reason` is a redacted
  diagnostic string.
- Overloading `Transferred` as also-success would make consumers (sweeps,
  recovery, settlement) distinguish "recovery owns the guard" from
  "preparation succeeded" by free-text reason — exactly the string-keyed
  dispositioning this lane spent 3c2 eliminating (exact-disposition rule).
  Any closure lens would call it WRONG.
- `BarrierSynced` is mid-flight progress (§2.5 steps 6–7: barrier published
  and synced, reopen-verify and effect admission still ahead), not settlement
  — which is why the slice-3 brief says no success-settlement state exists.
- Cost is at its floor NOW: the type is inactive — zero production writers
  (3d adds the first), zero readers, zero persisted instances, so the
  amendment is a pure additive variant with **goldens + serialization tests +
  exhaustive-match updates in the same change** (B21's amendment protocol;
  A2's goldens are the deliberate tripwire). After 3d ships a writer the same
  amendment becomes a migration.
- Naming: `Settled {}` (serde `"settled"`) follows the lane's settlement
  vocabulary (`UnusedSettled`, settle-dispatch).

## Scope (from the slice-3 brief §3d, unchanged)

(a) Claimed, non-cancellable materialization flight — first production writer
of `PreparationFlightStateV1`; runner retains map/provider/custodian `Arc`s
across caller-future drop; phase-distinguished cancellation tests (M13):
before-claim / after-claim-before-add / mid-add / after-add-before-evidence /
terminal-publication-failure, each with its expected durable state.
(b) Finite ownership (M3): `nonreturning_custody_sync_transfers_pre_effect_owner`
under manual `PreparationClockV1`; ZERO production timers (slice 4 arms).
(c) Candidate settlement (owner ruling): recovery-side `UnusedSettled`
producer; implementer designs the async/trait recovery seam (B18 — sweep is
sync, registration probe is async+private `host_git::registration_absent`),
boot-caller wiring, tri-state refusal (present / absent / cannot-prove →
refuse), as a design note the review checks. Refusing lock window across
proof→transition→unlink (B19; both-order contention tests; does NOT activate
the parked blocking-acquisition policy). Descriptor-safe removal (B20:
same-object descriptor-relative transition-then-unlink, no-follow,
parent-synced, crash-ordering + replacement/symlink negatives).
(d) The 2b2 marker population: marker-removal authority keyed on
state-agnostic exact-absence proof serves BOTH populations; NO table edge.

**Red-first battery (mandated):**
`unused_candidate_settles_only_after_exact_absence` (present-target refuses;
registered-but-absent refuses; both-absent settles, marker only);
dropped-configure-future per phase; the finite-ownership row; contention both
orders; replacement/symlink negatives.

## Anchors (measured on `6ad88565`)

- `crates/bridge-core/src/preparation_flight.rs:115` state enum, `:127` clock
- `crates/bridge-worktree/src/backend.rs:2327` `materialize_under_custody`
  (recovery-side transition doc at `:8271`)
- `crates/bridge-worktree/src/custody.rs:132` `UnusedSettled {}`; frozen
  transition table in the same module
- `crates/bridge-worktree/src/sweep.rs` both arms
- `crates/bridge-worktree/src/host_git.rs:114` `registration_absent`
  (async+private — the B18 seam), `:44` `cleanup_failed_add` (V3-forbidden)

## Dispatch shape (proposed)

- Estimate ~2,500 lines — the largest sub-slice. Propose THREE sequential
  tasks to keep each review round convergent (3c2 lesson):
  T1 = (B21 amendment) + (a) flight writer + M13 phase tests;
  T2 = (b) finite ownership + clock seams;
  T3 = (c)+(d) candidate settlement + lock window + descriptor-safe removal +
  marker authority.
  Each task: terra/xhigh via bridge, one counted Sol closure, cap one round +
  one targeted repair; STOP at 1.5× estimate (R3-6).
- Base: main (VERIFY local main == origin/main before dispatch — stale-ref
  gotcha 2026-08-15).
- Exit gate for the slice = slice-3 brief §6 rows that 3d owns:
  `unused_candidate_settles_only_after_exact_absence` green; both marker
  populations served; preparation-finiteness under manual clocks.

## T1 execution log (2026-08-16)

- T1 dispatched (impl-91867-53y2udsc, base `c37338dd`); 3-attempt bound
  reached with candidate `545103a4` (+890/−24: preparation_flight +20,
  worktree backend +821, handoff doc). Final verify PASS all four stages.
- Attempt-2 blockers (clippy red; durable `Failed` missing on Open-publication
  failure; collapsed red-first evidence) FIXED by attempt 3 — verified at
  source (the `Err(_) => publish_failed_after_initial_open_failure()` branch;
  per-test red-first entries in the handoff).
- Part 1 operator-inspected: `Settled {}` amendment exactly per B21 — doc
  comment carries the success-vs-transfer rationale; wire golden
  `{"state":"settled"}`; deny-unknown-fields negative; exhaustive-match
  comments updated.
- Final internal review REJECT, ONE blocker — CONFIRMED at source: no
  production caller-departure observation between durable `Open` and the add
  (only the cfg(test) `after_open_for_test` injected refusal, backend.rs
  ~:2714); the phase-2 test's real `configure.abort()` is causally inert —
  the asserted `Failed` comes from `hooks.fail_after_open` (the handoff's own
  red-first entry documents the caller-owned-runner mutation, not a
  drop-observation red). False-positive test + unimplemented contract.
- TARGETED REPAIR dispatched on frozen `545103a4` (host branch
  `feat/r2f1b-3d-t1`): R1 one-sample caller-departure check at the
  add-admission boundary (departed → typed Failed, zero add; else committed,
  phase-3 unchanged; no timers/watchers); R2 honest phase-2 test red-first on
  `545103a4`; R3 `fail_after_open` hygiene. Caps 120/300, single file.
  Counted Sol closure follows the repair.
- REPAIR DELIVERED first-attempt: `e66b9085` (impl-95834-kqsue52b; 109
  changed lines, single file + handoff; container verify PASS ×4; internal
  reviewer glitched → inconclusive, advisory only). Operator inspection:
  Drop-guard (Release) owned by the configure future w/ disarm-on-completion;
  ONE Acquire sample at the add-admission boundary; departed → typed durable
  `Failed{Canceled, bridge.worktree_preparation_caller_departed}`, zero add;
  present → committed, phase-3 unchanged; `fail_after_open` REMOVED; phase-2
  test drives the real abort and pins the typed code in the durable record.
- Operator red/green controls (host, run-verified; container red run was
  egress-blocked and disclosed in the handoff): phase-2 GREEN on `e66b9085`;
  RED under the single-line sample-severing mutation with exactly the
  predicted `left: Some("settled")` / `right: Some("failed")`.
- Gates on exact `e66b9085`: fmt clean; workspace clippy `-D warnings`
  clean; full suite **4,117/0/13 across 90** (baseline 4,111/0/13).
- Counted Sol closure DISPATCHED on full `c37338dd..e66b9085` (sol/max,
  solmax config). Host branch `feat/r2f1b-3d-t1` = `e66b9085`.
- COUNTED SOL CLOSURE: **REJECT, 2 WRONG-BLOCKER / 1 SMELL-DEFER**
  (verbatim: reviews/2026-08-16-r2f1b-3d-t1-sol-closure.md, commit
  155e2d76). W1: ConfigureAdmission::Drop cleanup never joins a committed
  flight → pops Reserving, CellContended not reinserted, runner `_ => {}`
  silently accepts → LiveProtected+Settled with preservation identities
  permanently lost; second schedule pre-`enter`. W2: post-departure terminal
  publication failure discarded with the dead oneshot; owner removed;
  durable BarrierSynced nonterminal, no diagnostic. SMELL: unbounded test
  liveness waits (fold into repair). Closure VALIDATED: B21 amendment sound;
  sample linearization sound (no bad schedule at the boundary itself);
  phase-2 discriminating. Phases 3-4 false-positive for map/identity
  retention; phase 5 misses the detached-receiver edge.
- CLOSURE REPAIR dispatched on frozen `e66b9085` (the declared cap's one
  targeted repair): R1 cleanup joins/defers to committed flight through
  projection + loud missing-entry; R2 backend-owned joinable completion/debt
  record surfacing detached terminal failures; R3 bounded test waits. Caps
  450/650, single production file. Bounded Sol re-look on the repair delta
  follows; then land.
- PIPELINE INCIDENT (credential class, diagnosed to mechanism): repair-2
  dispatches a+b died pre-checkpoint ("workflow did not complete", NO
  commit; agent container killed 1 s after start, exit 137 = bridge
  teardown). Root cause chain, each link probed: (1) `models` probe isolated
  it to the impl agent — `spawn codex-acp: Invalid params`; (2) RUST_LOG
  trace pinned the -32602 to the `authenticate(methodId="chat-gpt")` call;
  (3) auth.json (bridge cred copy) was rewritten 02:16 by the last
  successful run and now carries OPENAI_API_KEY alongside the ChatGPT token
  family; the containerized codex-acp now advertises EMPTY authMethods
  (already-authed) so ANY authenticate call is rejected; (4) commenting
  `auth_method` alone did NOT stop the bridge from sending authenticate
  (falsified probe, logged); (5) `pre_authenticated = true` (the knob the
  working solmax config uses) FIXED it — impl probe now lists the full
  terra/sol roster. Config edit committed; the what-wrote-OPENAI_API_KEY
  question joins the single-token-family credential ledger item. Sol-lens
  probes were UNAFFECTED throughout (pre_authenticated already set there).
- Repair-2 re-dispatched with the fixed config (same frozen spec; the two
  dead clones impl-42693/impl-42978 left for the reaper — no commits, no
  checkpoints).
- CLOSURE REPAIR DELIVERED (after the credential incident): `6e6ad453`
  (impl-44659-l0tumoxb, first attempt, verify PASS ×4, internal reviewer
  glitched again → advisory-inconclusive). Design: cleanup joins a
  backend-owned watch completion record of COMMITTED (or errored) flights
  after configure drains, before entry_for_cleanup can pop; commitment =
  one CAS raced by the caller guard and runner; terminal-write failure
  persists as typed backend debt consumed by cleanup/retirement; `_ => {}`
  projection → typed AgentCrashed; test waits bounded 2 s. 595 changed
  lines (>450 soft, <650 hard, disclosed).
- Operator controls (host, run-verified; container reds egress-blocked
  again): 3 greens; Red A (join severed) both R1 red at the join latch;
  Red A2 (wait skipped, latch kept) both R1 red at the joinable-cleanup
  expectation; Red B (completion omitted + unconditional removal) R2 red
  losing the typed StoreFailure. Divergence from the handoff's predicted
  final-assertion shapes DISCLOSED to the re-look.
- Gates on exact `6e6ad453`: fmt/clippy clean; full suite **4,120/0/13
  across 90**.
- BOUNDED SOL RE-LOOK: **APPROVE — W1 FIXED both schedules (CAS at :217 +
  subscribe-before-pop at :2351), W2 FIXED (typed debt; conditional
  removal at :3061; retirement inventory at :4160), join scope SOUND;
  red-shape divergence ruled no-ledger-item.** 2 SMELL-DEFERs ledgered:
  R1a-latch/R2-retirement waits still unbounded; explicit pre-sample
  non-join coverage (~15–25 lines, red = join-every-flight mutation).
  Verbatim: reviews/2026-08-16-r2f1b-3d-t1-sol-relook.md.
- **T1 ROUND CLOSED — every WRONG fixed. PR #53 opened** (branch
  feat/r2f1b-3d-t1 = 6e6ad453); CI watch running; rebase-merge on green.
