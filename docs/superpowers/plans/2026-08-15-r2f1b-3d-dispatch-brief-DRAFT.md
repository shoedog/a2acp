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
- T1 LANDED: PR #53 rebase-merged → main `42dd555a`; local main ff'd;
  19.2G T1-round targets reaped (receipt in session; clones retained).
- POST-MERGE CI: Build/Lint/Coverage RED on ONE pre-existing whole-bin test
  (`compatibility::tests::staged_candidate_exec_is_bound_to_the_verified_file_object`,
  `smoke_process_launch_failed` = process-launch environmental failure).
  Classification LOAD-FLAKE, new coverage-lane instance class: identical
  content green ×2 on the PR runs minutes earlier, pre-T1 main green, host
  suite green, T1's 3-file diff doesn't touch compatibility/smoke machinery.
  Rerun of the failed job dispatched for a fresh same-SHA control. The
  coverage-lane flake family now has two ledgered classes
  (authority_mutation lock-release; staged-candidate smoke launch).
- T2 DISPATCHED in parallel (base `42dd555a`; spec 81fc9472); its fold gate
  holds until the rerun resolves green.
- Rerun GREEN on exact `42dd555a` — same-SHA control confirms the
  load-flake classification; main CI green; T1 fully closed. T2 fold gate
  clear (T2 round still in flight).

## T2 execution log (2026-08-16)

- T2 delivered FIRST DISPATCH: `c5d9390c` (impl-90946-s08a7nvl; 682 changed
  lines, >500 soft <800 hard, disclosed; verify PASS ×4; internal reviewer
  glitched a THIRD consecutive time → inconclusive-advisory; recurring
  pipeline note for the ops ledger).
- Operator inspection: zero production arming is STRUCTURAL (production
  claim() has no bound parameter); transfer path = CAS begin_transfer →
  durable Transferred{reason w/ 30s/31s values} → typed ConfigInvalid
  refusal → owner (owning the runner JoinHandle) moved into
  preparation_recovery_flights (T3 = first consumer). Handoff population
  table rules identity capture post-BarrierSynced non-transferring.
- Host controls: 5/5 greens; mutation severing BOTH production boundary
  checks reddens the journal-open test (production boundaries honestly
  covered) but the NAMED EXIT-GATE nonreturning test STAYS GREEN — its
  transfer is driven by cfg(test) observe_preparation_bound_for_test
  (~:1921). Framed for the closure as the round's main question:
  (a) production observation seam required NOW (T1-phase-2 class) vs
  (b) correctly deferred to slice-4 arming w/ a binding obligation.
- Gates on exact `c5d9390c`: fmt/clippy clean; full suite **4,125/0/13
  across 90** (baseline 4,120).
- Counted Sol closure DISPATCHED (sol/max) with the (a)/(b) question and
  all control evidence disclosed.
- COUNTED SOL CLOSURE: **REJECT, 4 WRONG-BLOCKER / 2 SMELL-DEFER**
  (verbatim: reviews/2026-08-16-r2f1b-3d-t2-sol-closure.md, commit
  4f8c4eb7). W1 transfer/barrier on two independent atomics — Settled can
  overwrite durable Transferred, both race orders; W2 action bound sampled
  only pre-op — slow-returning op admits effects; W3 nonreturning INITIAL
  journal op has no journal to transfer into — irreversible begin_transfer
  then StoreFailure, configure stuck; W4 completion published before
  recovery insertion — T3/retirement can miss the owner. SMELLs: runner-exit
  sender guard; transfer terminal-publication-failure regression. THE
  OBSERVER QUESTION RULED (b) DEFERRED with a BINDING slice-4 obligation
  (inject bound + 31s control wake + call the corrected phase transition +
  end-to-end nonreturning test w/o the cfg(test) observer). Identity-capture
  post-barrier ruling SUSTAINED. Operator's exit-gate mutation evidence
  incorporated (its green traced to the cfg(test) trigger; deferral covers
  the trigger, NOT findings 1-4).
- CLOSURE REPAIR dispatched on frozen `c5d9390c` (the cap's one repair):
  single CAS phase enum (Preparing → TransferPublishing|BarrierPublishing);
  post-return clock samples; pre-established terminal-capable control
  journal for the initial-op window; recovery-insert-before-completion;
  s1/s2 folds. Caps 600/900. Bounded re-look follows.
- CLOSURE REPAIR DELIVERED at the 3-attempt bound: `435257ce`
  (impl-37431-pai85310; verify PASS ×4; internal reviewer failed AGAIN —
  Authenticate timeout, the synth itself ruled the probe inadmissible;
  ROOT-CAUSED: host codex reviewer had the same credential class → config
  fix `9a941531` adds pre_authenticated; 4 lost internal reviews ledgered).
  Design: sticky phase CAS Preparing→TransferPublishing|BarrierPublishing;
  failed Transferred write stays TransferPublishing debt; transfer =
  durable-terminal → recovery-register(no-replace) → active-remove → wake;
  control journal opened pre-runner (control dir, replace-exact-Open).
  903 changed lines — 3 OVER the 900 hard cap, disclosed.
- Operator controls: 6/6 regressions green; W2 red (post-return sample
  severed) deterministic at the typed-refusal assertion; remaining
  prescribed reds are multi-line — delegated to the re-look's source
  verification, disclosed.
- Gates on exact `435257ce`: fmt/clippy clean; full suite **4,131/0/13
  across 90** (baseline 4,125).
- BOUNDED SOL RE-LOOK dispatched (W1-W4 + s1/s2 + the T1-composition
  question the closure left open + the W3 control-journal design).
- BOUNDED RE-LOOK: **REJECT — CONVERGING** (verbatim: reviews/
  2026-08-16-r2f1b-3d-t2-sol-relook.md, b2df332f). W1/W2/W4/s2 FIXED and
  sustained; s1 folded w/ abort residue DEFERRED. Surviving population, all
  closed: E1 pre-barrier FAILURE writers (departure/custody-error/runner-
  exit) bypass the phase CAS — check-then-publish can clobber a durable
  Transferred (T1-composition answered: every terminal writer must claim
  the phase); E2 W3-not-fixed-at-root — the control-journal OPEN is itself
  the first stall-capable op, now stalling with NO owner published; E3
  exact-Open replacement is advisory TOCTOU (fs_custody replacing rename
  clobbers). Plus the 903-line cap breach ruled a contract SMELL-blocker.
- **DISCLOSED CONVERGENCE EXTENSION (one line, per the owner-promoted
  discipline): the T2 cap (one round + one repair) is extended by ONE
  second targeted repair on the frozen artifact — classification
  CONVERGING (4 of 6 findings fixed and sustained; remainder closed,
  enumerable, non-repeating), a binding second look follows, and a
  non-converging result PARKS the slice and escalates to the owner.**
- Extension repair dispatched on frozen `435257ce`: E1 failure-arm in the
  same phase CAS claimed by every pre-barrier terminal writer; E2
  pre-owned control root + publish-flight-before-blocking-ops; E3 atomic/
  refusing terminal replacement (narrow fs_custody primitive authorized if
  needed, Task-C precedent); E4 caps 500/750 hard-respected. s1 abort
  residue + slice-4 observer obligation unchanged.

## T2 PARKED — owner escalation (2026-08-16)

The disclosed convergence extension did NOT converge: the extension repair
(impl-85729-fb3gv2wd) reached its 3-attempt bound at `f66016e0` with its
OWN three new regressions red on its own head
(failure_owned_runner_exit_completes_configure_result,
preparation_control_root_refuses_identity_replacement,
terminal_replacement_serializes_exact_open_writers — bridge-worktree lib
270/3), verify FAIL at test, and the internal review ruling E2 undelivered
(root-pin failure orphans the active flight; per-flight blocking waits
persist). Per the declared extension boundary: PARKED, no further rounds
without owner word.

**State of record:**
- main = `42dd555a` (T1 landed; production untouched by T2; V3 unarmed).
- Last fully-gated T2 artifact: `435257ce` on `feat/r2f1b-3d-t2` (pushed) —
  re-look ruled W1/W2/W4/s2 FIXED there; 3 surviving blockers (E1 phase
  bypass by failure writers, E2 root-open stall, E3 exact-Open TOCTOU), all
  closed-class, all production-unreachable today (bounds unarmed; no
  transfer trigger exists in production).
- Parked extension candidate: `f66016e0` on
  `salvage/r2f1b-3d-t2-extension-candidate` (pushed) — E1/E3 partially
  built, 3 of its own tests red; E2 undelivered.

**Owner options:**
1. Authorize a second extension: one more targeted repair, either finishing
   `f66016e0` (its reds may be mechanical) or restarting the extension from
   frozen `435257ce`.
2. Park-and-redesign: re-scope E1/E2/E3 as their own sub-slice with a fresh
   cap and design pass (the steering's escalate-to-design path) — E2's
   ownership ripple is the strongest argument here.
3. Hold T2 unlanded and let T3 planning proceed on paper only (T3 depends
   on T2's recovery inventory, so no T3 dispatch before T2 resolves).

Carried regardless: s1 abort residue DEFER; slice-4 binding observer
obligation; the 4-lost-internal-reviews credential note (fixed 9a941531);
cap-breach hygiene note (903>900) on the first repair.

## T2 UNPARKED — owner authorized a second extension (2026-08-16)

Owner authorized "an extension" on the parked T2. Operator (opus) took
option 1 and, per the convergence discipline's no-restart clause, continued
on `f66016e0` rather than restarting from `435257ce` — a restart would have
discarded delivered E1/E2/E3 work with no evidence the artifact was
unsalvageable.

**The park's evidence was re-examined at source and largely does not hold.**

- **The three reds are MECHANICAL, not design.** `unique_temp_dir()` only
  computes a path; it never creates the directory (every other caller
  creates it via `provider_fixture`/`backend_fixture`). All three new tests
  construct the control root against a nonexistent path — two fail at
  `open_claimed_for_session_admission()` with `StoreFailure`, the third at a
  non-recursive `std::fs::create_dir(&root)` with `NotFound`. Adding
  `std::fs::create_dir_all(&tmp)` to the three takes bridge-worktree lib
  from **270/3 to 273/0** (host, run-verified, worktree `.claude/worktrees/t2ext`).
- **Gates on `f66016e0` + that 3-line harness fix (host, run-verified):**
  `cargo fmt --all -- --check` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; bridge-worktree lib 273/0. Whole
  workspace run surfaced ONE failure outside T2's scope —
  `cli_tests::guarded_spawn_ignores_retargeted_static_cwd_for_native_mcp` in
  `bin/a2a-bridge` — which PASSES in isolation on the same tree; T2's diff
  touches neither the bin nor MCP spawn code, and this repo carries a known
  whole-bin parallel-flake class (`fix/whole-bin-parallel-flakes`).
  Classified parallel-load flake; a full `--no-fail-fast` count is the
  fold-gate control.
- **The pipeline was NOT broken.** The impl agent's `a2a-lf` HTTP 403 is the
  implement-lane egress allowlist working as designed (ADR-0013): the
  implement proxy permits model APIs ONLY, crates.io is deliberately absent,
  and the dependency-capable verify container is the compile lane. The agent
  therefore had no local compile loop — which is why a trivial harness
  omission survived three attempts. Not a regression; a standing constraint
  now stated explicitly in the repair spec.
- **Line cap held**: `git diff --numstat 435257ce..f66016e0` = 736 changed
  lines against the extension's 500 soft / 750 hard. No breach this time.
- **E2's core IS delivered**: the active flight is inserted into
  `preparation_flights` before the root pin is claimed, and the blocking open
  is moved into a detached `spawn_blocking`, so an observer finds and can
  terminalize the exact owner during the stall (proven by
  `stalled_control_root_pin_is_observable_before_terminalization`).

**ONE real WRONG survives, and it is closed and enumerable.** Proven on the
host with a diagnostic probe (arm the nonreturning root pin, remove the
control root while blocked, release):

```
owner published before the blocking pin = true
first  configure = Err(StoreFailure)          <- correct
entry retained after failure = true           <- THE DEFECT
second configure = Err(AgentOverloaded)       <- permanent, process lifetime
```

The runner's `root_ready` error arm completes the caller but calls
`runner_exit_guard.complete()`, disarming the only path that removes the map
entry (`terminalize_preparation_runner_exit`). The reservation leaks, and the
admission check refuses every later configure for that session. It leaks for
every flight parked on the same failed pin, not just the one that claimed it.
The review's other residual — per-flight blocking waits persist — names no
incorrect output and is a **SMELL**, deferred with a ledger entry.

**DECLARED CAP FOR THIS ROUND (declared before dispatch):** ONE targeted
repair on frozen `f66016e0` + ONE bounded Sol re-look on the repair delta.
Caps 150 soft / 250 hard changed lines, production confined to
`backend.rs`. If this round does not converge, T2 goes to option 2
(re-scope E1/E2/E3 as a designed sub-slice) — there is no third extension.

- Repair spec written: `plans/2026-08-16-r2f1b-3d-t2-extension-repair2-task.md`
  (R1 harness fix; R2 reservation release composed with E1's phase claim,
  guarded by `Arc::ptr_eq`, publishing no durable record because the control
  root is precisely what failed; red-first T-A/T-B/T-C).
- **FULL-WORKSPACE CONTROL on clean `f66016e0`** (host, unloaded,
  `cargo test --workspace --no-fail-fast`) — never run before this round,
  because the implement container structurally cannot compile and fable's
  host runs were scoped to `bridge-worktree --lib`: **4,133 passed / 3 failed
  / 13 ignored across 90 targets**, and the ONLY three failures in the entire
  workspace are the three mechanically-broken new tests. Zero regression
  anywhere else in the tree (baseline `435257ce` = 4,131/0/13 across 90; the
  extension is +5 tests net). The whole-bin parallel flake seen under load
  (`guarded_spawn_ignores_retargeted_static_cwd_for_native_mcp`) did NOT
  recur in this unloaded run, confirming the load-flake classification.
  Evidence: scratchpad `t2ext-baseline-nff.log`.
  Net: the parked artifact is three test-harness lines away from a fully
  green workspace suite, plus the one proven WRONG that R2 closes.
- Dispatch 2 running: clone `impl-91809-nf10irod`, base
  `salvage/r2f1b-3d-t2-extension-candidate` (`f66016e0`), terra/xhigh,
  `--depth light --strict-brief`.
