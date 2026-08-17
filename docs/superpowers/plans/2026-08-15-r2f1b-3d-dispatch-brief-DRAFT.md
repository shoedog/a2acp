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

## T3 — SPLIT into T3a/T3b; T3a execution log (2026-08-17)

**SPLIT DECISION (operator, before dispatch).** §3d(c)+(d) bundles the absence
proof, an async/trait seam, a refusing lock window, a descriptor-safe removal
and a two-population marker authority into one task. T2 was 682 delivered lines
and consumed four review rounds, a park, an extension and two operator
completions; (c)+(d) is larger. The standing discipline is to split before
dispatch rather than after rejection, so:

- **T3a DECIDES** — the state-agnostic exact-absence proof, the B18 seam, the
  fail-closed tri-state, the recovery-inventory coupling, boot wiring. **Zero
  record mutation, no transition-table edge.** Effect-free is safe to land by
  construction (`custody_writer.rs:20-27` precedent).
- **T3b ACTS** — the refusing lock window across proof→transition→unlink, the
  `UnusedSettled` transition, descriptor-safe removal (B20), and the
  two-population marker authority.

**DECLARED CAP for T3a (before dispatch):** one pre-closure targeted repair on
operator-verified findings → ONE counted Sol closure → at most ONE targeted
repair on closed enumerable findings. Non-convergence parks and escalates; no
silent extension.

- T3a dispatched (`impl-41288-epg2lw7h`, base `main` `1d7826dd`, terra/xhigh).
  Reached its 3-attempt bound at `c336d9c7`, **verify PASS ×4** (fmt, clippy,
  build, test). Internal review functioned this round (three consecutive
  glitch-free rounds after the `pre_authenticated` fix) and rejected each
  attempt with a *different* real finding — attempt 1 compile/lint + a recovery
  race, attempt 2 a symlink-following probe, attempt 3 the sidecar guard.
  699 changed lines, inside the 750 cap. Preserved on pushed branch
  `salvage/r2f1b-3d-t3a-first`.
- **Operator verification at source on `c336d9c7`:**
  - DELIVERED: the B18 seam + `ExactAbsenceProbeV1`; the tri-state; the
    recovery coupling (`decide_unused_candidate_for_recovery`, `backend.rs:2271`)
    with a passing test; population sharing; effect-free by construction.
  - **Attempt 2's symlink finding is FIXED** — `target_absent_from_probe`
    (`host_git.rs:108`) uses `symlink_metadata()`, so a dangling symlink reads
    as present. Verified at source, not taken on the reviewer's word.
  - **SURVIVING WRONG (confirmed):** `sweep_orphans_with_exact_absence`
    (`sweep.rs:460`) builds its candidate straight from a
    `ScannedWorktreeRecordV1::Legacy` sidecar without the
    `sidecar_file_matches` / `worktree_under_root` guards `sweep_orphans`
    applies, so a forged or stale `*.meta.json` naming an out-of-root absent
    path can reach `Authorized`. Effect-free today, but the decision value IS
    T3a's deliverable and T3b will act on it.
- **THE REVIEWER'S SECOND CLAIM, CORRECTED.** It said the handoff "lacks the
  mandated exact pre-change failure output". The handoff HAS a red-first
  section — but every entry is a **compilation error**
  (`error[E0425]: cannot find function ...`). A compile error proves only that
  an API did not exist; it is not evidence a test discriminates behavior. So
  AC 7 is unmet in substance while the reviewer's wording was imprecise.
- **ROOT CAUSE IS THE OPERATOR'S SPEC, and it is structural.** An implement
  agent on this lane has **no compile loop** (implement-lane egress is model
  APIs only, ADR-0013). It therefore *cannot* produce behavioral red-first
  evidence — only compile-error "reds". Demanding it in acceptance criteria
  invites exactly the misleading transcript that appeared. **Standing
  correction: the OPERATOR owns discriminating red/green controls on the host;
  the agent is asked for an honest "not run, no local toolchain" plus the
  mutation that should redden.** The T3a repair spec now says this outright.
- Pre-closure targeted repair dispatched on frozen `c336d9c7` for the ONE
  surviving WRONG (caps 200 lines, single file). Operator host controls run in
  parallel.
- GOTCHA, second occurrence: a fenced ```` ``` ```` Commit Message block in a
  task spec gets copied literally, so the agent commits a bare ``` subject
  (`a84c8b57`, now `c336d9c7`). The repair spec states the subject line
  unfenced instead.

### T3a repair 1 + the operator host controls that changed the picture

- Repair 1 CONVERGED (`impl-45652-ewibypmp`, `87a03ed0`): verify PASS ×4,
  internal review **APPROVE**, sidecar guards reused rather than reimplemented.
  199 lines against the 200 cap.
- **Out-of-scope rider caught and REVERTED (`5cbfddf2`).** The repair, scoped to
  `sweep.rs`, also rewrote `observe_container_identity` in
  `bridge-core/src/reaper.rs` — swapping drop-based `kill_on_drop` for an
  explicit `child.kill().await` on timeout — to chase
  `reaper::tests::production_timeout_kills_child_before_delayed_side_effect`
  failing in container verify. That test PASSES on the host in both full-suite
  runs this session. It is the process-spawn-under-load flake profile, and
  because this instance is in the CONTAINER VERIFY lane rather than the coverage
  lane, **the family is not coverage-specific** — a correction to the framing in
  `reviews/2026-08-17-coverage-lane-flake-family-investigation.md`. The change may
  still be a real fix (`kill_on_drop` signals without awaiting, so a delayed side
  effect can race a dropped future); filed as its own follow-up rather than
  landed as a silent rider.
- **THE CONTAINER'S `verify: PASS ×4` DID NOT HOLD ON THE HOST.** Operator bench
  at frozen `c336d9c7`: two of the four `exact_absence` tests FAIL, and one of
  them HUNG the suite before the bounded-gate follow-up was cherry-picked in.
  Applying that follow-up converted the hang into a clean 30 s diagnosable
  failure — the fix earning its keep within the hour, and the reason both
  defects below are legible at all.
- **Finding 1 — WRONG, fails OPEN.** `host_git.rs:448`,
  `left: BothAbsent / right: RegisteredButAbsent`.
  `registration_absent_sync` compares `candidate.worktree_path` byte-for-byte
  against the path git prints. Git prints its CANONICAL path; the candidate
  carries its original spelling. Verified against the host toolchain (git
  2.50.1): after `rm -rf` of the worktree dir, `git worktree list --porcelain`
  still lists it as `prunable` at `/private/var/…` while the candidate holds
  `/var/…`. Unmatched ⇒ "registration absent" ⇒ with an absent target ⇒
  `BothAbsent` ⇒ **Authorized**. A fail-closed proof failing open. THIRD
  instance of raw-vs-canonical path divergence in this lane (T2's control root
  was the second). The container cannot see it: Linux has no `/var`→`/private/var`
  indirection, so the textual compare happens to match there.
- **Finding 2 — WRONG.** `backend.rs:11971`,
  `left: Authorized / right: Refused(CannotProve)` — a recovery-owned candidate
  is authorized. This is precisely the coupling that made T3 depend on T2, and
  it is the finding attempt 1's internal review raised; it was NOT actually
  fixed. Before the bounded gate it presented as a hang, not a failure.
- **DISCLOSED CAP EXTENSION (one line, per the discipline): T3a's declared
  "one pre-closure repair" is extended by ONE second pre-closure repair.**
  Classification: these are GATE failures surfaced by controls that did not
  exist when the cap was declared, not review findings from a counted round;
  both are closed, enumerable and non-repeating; and sending a
  not-host-green artifact into the counted Sol closure would waste the counted
  round. A non-converging result parks T3a and escalates.
- Repair 2 dispatched on frozen `5cbfddf2` (caps 300 lines) for both findings,
  with the container-verify-is-not-sufficient warning stated in the spec.

### T3a repair 2 + operator completion — HOST GREEN, counted closure dispatched

- Repair 2 CONVERGED in 2 attempts (`impl-50263-tbkfenrg`, `2e4dfb37`): verify
  PASS ×4, internal review APPROVE, 277 lines against the 300 cap. Attempt 1 was
  rejected only for unused imports under `clippy -D warnings`; the reviewer
  traced the core logic and found no correctness defect.
- **Finding 2 (recovery-owned Authorized) FIXED** — host-verified.
- **Finding 1 (byte-exact path comparison) FIXED** by a shared
  `paths_resolve_to_same_identity` comparator used by both the sync and async
  probes, plus an `unresolvable_registration_paths_refuse_exact_absence` test.
- **THE CONTAINER'S PASS FAILED TO HOLD A SECOND TIME.** Host bench at
  `2e4dfb37`: 7 of 8 `exact_absence` tests green, one still red. Root-caused to
  a **TEST-fixture defect, not production** —
  `synchronous_exact_absence_capability_distinguishes_all_host_observations`
  builds a deliberate symlinked worktree root, then derives
  `canonical_worktree_root` from `std::env::temp_dir()`, which on macOS is
  ITSELF symlinked (`/var` → `/private/var`). So the value named "canonical"
  resolved the fixture's own symlink hop but not the platform's, and the test
  compared `/var/…` against git's `/private/var/…`. Verified git's actual
  behavior against the host toolchain (2.50.1): git ALWAYS records the fully
  canonical path, including when the worktree is added through a non-canonical
  absolute path. Linux `/tmp` is not symlinked — which is why the container
  passed this test twice, the same environment-masking that hid the production
  defect the test exists to catch.
- **OPERATOR COMPLETION `b255cba5`**: resolve the fixture root. One line plus
  the reasoning. The test then passes, which is the confirmation that the
  comparator, the fail-closed probes and the recovery refusal are all correct
  through a symlinked root.
- **GATES GREEN on exact `b255cba5`** (host, unloaded): fmt clean; workspace
  clippy `-D warnings` clean; full suite **4,149 passed / 0 failed / 13 ignored
  across 90 targets** (+9 tests over `main`). Non-unix gate N/A — T3a touches
  zero `bridge-core` files, and the Windows job compiles only
  `bridge-store` → `bridge-core`.
- Pushed: `salvage/r2f1b-3d-t3a-complete` = `b255cba5`.
- **Counted Sol closure dispatched** on the full `1d7826dd..b255cba5` delta.

**SIZING LESSON, recorded against my own estimate.** I split T3 specifically so
each half would be convergently sized, and estimated T3a at 450–600 lines. It
came in at **1,106** — roughly double. The split was still right (undivided
(c)+(d) would have been ~2,000), but the estimate was not, and the counted round
is again reviewing more than the discipline's own advice would like. For T3b:
size from the delivered T3a delta, not from the brief's prose.

**PROCESS NOTES against myself this round:**
- I wrote `./tools/check-nonunix.sh | tail -2; echo "EXIT=$?"`, which captured
  `tail`'s exit — so a missing script reported a false `exit 0`. That is exactly
  the "prints errors, exits 0" failure I had warned about one round earlier.
  Harmless here (the gate is N/A) but it reported success without running.
- The commit-subject defect recurred a THIRD time and is now root-caused: the
  typed task-spec schema treats the entire `## Commit Message` section as the
  message, so instruction prose placed inside it becomes the subject. First a
  fence produced a bare ```; then a warning sentence produced the warning
  sentence. **The section must contain the message and nothing else**; guidance
  belongs outside it. Fix this in the T3b spec.

## T3a PARKED at the path-identity boundary — owner escalation (2026-08-17)

Counted Sol closure on `1d7826dd..b255cba5` (1,106 lines): **REJECT, 3 WRONG /
3 SMELL-DEFER** (verbatim: `reviews/2026-08-17-r2f1b-3d-t3a-sol-closure.md`).
All three operator-verified at source:

1. The real 2b2 V3 population never reached the proof — `sweep.rs:564` refused
   every V3 record claiming the schema carries no source. **That claim is false:**
   `custody.rs:196` makes the claim `Required` for `PreservationUnknown`, and
   `PreservedWorktreeClaimV1` carries `source`/`root`/`worktree`/`common_dir`.
   The "serves both populations" test fabricated its second population, so it
   was a false positive as well as a delivery gap.
2. Unchecked candidate strings reach `git -C`, so a legacy sidecar with a
   relative source queries whatever repo the bridge launched in and authorizes.
3. Missing-tail byte equality is not identity: on a case-insensitive filesystem
   `/root/wt` and `/root/WT` compare unequal and authorize.

**Repair 3 (`impl-4355-lyhk4i8p`, `ad60db53`) did NOT converge** — 3-attempt
bound, verify FAIL at test, review REJECT. Exactly 600 changed lines against a
600 hard cap, and no handoff update. Delivered R1 and R3; **R2 remains unmet and
R3 is over-conservative**:

- `ExactAbsenceCandidateV1::from_claim` takes `common_dir` as `_common_dir` and
  **drops it**. Replace only the `source/.git` common-dir object while leaving
  the source inode intact: `revalidate_source()` passes, `git -C` queries the
  replacement repo, and T3a authorizes a target still registered under the
  original. That is the fifth path-identity instance.
- The tri-state comparator refuses EVERY missing-tail comparison, which breaks
  the pre-existing `porcelain_registration_check_is_exact_and_handles_locked_records`
  (host suite 289/1). Its data is synthetic: listed `/repo` and `/managed/wt`,
  queried `/managed/other`, none of which exist on disk. Clearly-distinct names
  ought to be a PROVEN difference; refusing them means the proof can never
  authorize whenever the repo holds any other registration. Fail-closed, but
  functionally inert — and it silently changes the semantics of the existing
  removal-verification path that shares the parser.

**PARKED, per the boundary I declared myself.** The sub-slice note written
earlier today says: *"If a fifth instance appears, stop patching and build the
primitive here."* It has appeared. The owner's standing authorization covered
one more repair round; it was spent on repair 3. Taking a fourth would exceed
both that authorization and my own declared rule, so this goes to the owner.

**State of record:**
- `main` = `1d7826dd` (T1+T2). T3a has never touched it.
- Last HOST-GREEN T3a artifact: `b255cba5`
  (`salvage/r2f1b-3d-t3a-complete`) — fmt/clippy clean, 4,149/0/13 across 90,
  carrying the closure's three WRONGs.
- Parked repair: `ad60db53` (`salvage/r2f1b-3d-t3a-repair3`) — R1 + R3
  delivered, R2 unmet, host suite 289/1.

**Owner options:**
1. **Build the path-identity primitive as its own designed sub-slice** (the
   widened `plans/2026-08-16-r2f1b-3d-t2-root-identity-subslice.md`), then
   finish T3a on top of it. Operator recommendation: five instances across two
   slices, three of them failing open, and the remaining question — *when are
   two possibly-nonexistent paths provably different?* — is precisely the
   primitive's design question, not a repair detail.
2. **Authorize a fourth, narrowly scoped repair**: bind `common_dir`, and make
   the comparator prove difference for clearly-distinct names (deepest existing
   ancestors differ ⇒ different; same ancestor ⇒ compare remaining components
   under the filesystem's own case semantics; only case/normalization aliases
   are ambiguous). Both items are enumerable, so this is defensible if momentum
   matters more than the boundary.
3. **Land R1 alone** — take the V3-population fix onto the host-green
   `b255cba5` base, revert R2/R3, and carry the path-identity findings to the
   sub-slice. Smallest landable increment; leaves two known fail-open holes in
   an unreachable path.

Carried regardless: the three closure SMELLs (unbounded sync I/O on async boot
paths; regression evidence not behaviorally fail-first; handoff cap
unreconciled), and T3b remains unstarted.
