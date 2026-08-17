# R2f1b 3d T2 — extension repair 2: dispatch declaration (2026-08-16)

Owner authorized a second extension on the parked T2. This brief declares the
round before dispatch, per convergence discipline.

## Why the park was lifted

The park rested on "the extension repair reached its 3-attempt bound with its
own three regressions red." That evidence was re-examined at source and does
not support a design-level failure:

- All three reds are ONE mechanical omission — `unique_temp_dir()` computes a
  path and never creates the directory, so the three new tests construct the
  control root against a nonexistent path. Host-verified: adding
  `std::fs::create_dir_all(&tmp)` to the three takes `bridge-worktree --lib`
  from 270/3 to **273/0**, with fmt and workspace clippy `-D warnings` clean.
- The agent's `a2a-lf` HTTP 403 is the implement-lane egress allowlist working
  as designed (ADR-0013: model APIs only; crates.io deliberately absent). The
  agent had no local compile loop — which is why a trivial harness omission
  survived three attempts. Per the evidence-admissibility discipline, a probe
  that failed for its own reasons yields no evidence about the hypothesis.
- E1, E2's core, and E3 are delivered. `git diff --numstat 435257ce..f66016e0`
  = 736 lines, inside the extension's 500/750 caps (no breach this round).

Per the convergence discipline's no-restart clause, the round continues on
`f66016e0` rather than restarting from `435257ce`: a restart would discard
delivered work with no evidence the artifact is unsalvageable.

## Scope — two findings, both closed

| Id | Class | Finding |
|----|-------|---------|
| R1 | mechanical | three new tests build a control root against a path `unique_temp_dir` never created |
| R2 | **WRONG** | a failing control-root pin permanently orphans the preparation reservation |

R2, proven on the host by driving `arm_nonreturning_control_root_pin`,
removing the control root while blocked, then releasing:

```
owner published before the blocking pin = true    <- E2's core IS delivered
first  configure = Err(StoreFailure)              <- correct
entry retained after failure = true               <- THE DEFECT
second configure = Err(AgentOverloaded)           <- permanent, process lifetime
```

Mechanism: the runner's `root_ready` error arm completes the caller, then calls
`runner_exit_guard.complete()`, disarming `terminalize_preparation_runner_exit`
— the only path that removes the `preparation_flights` entry. Every flight
parked on the same failed pin leaks its own reservation, not just the claimant.
The same arm also completes unconditionally, without consulting the phase, so a
transfer that claimed the terminal during the blocked pin can be completed over
(T-B covers this).

Non-scope, deferred with ledger entries: per-flight blocking waits on the root
pin (**SMELL** — a bounded resource concern naming no incorrect output); s1
abort residue; the slice-4 binding observer obligation.

## Dispatch

- Route: `a2a-bridge implement` (owner rule 2026-08-09), config
  `examples/a2a-bridge.r2f1b-impl.toml` (impl = gpt-5.6-terra @ xhigh — terra
  implements because sol is the review lens), `--lang rust`, `--depth light`
  (the counted re-look is the real gate), `--base-ref
  salvage/r2f1b-3d-t2-extension-candidate` (= `f66016e0`), `--strict-brief`.
- Task spec: `plans/2026-08-16-r2f1b-3d-t2-extension-repair2-task.md`.
- First dispatch was REFUSED by the typed task-spec gate (missing required
  `Acceptance Criteria`) and flagged by brief-lint (`premise-without-license`
  on the 270→273 host result). Both fixed before re-dispatch; the spec now
  carries an explicit falsification license telling the agent to report a
  mismatch rather than force the change.
- The spec states the no-crates.io constraint outright so the agent does not
  spend attempts on an environment it cannot fix.

## Declared cap (before dispatch)

**ONE targeted repair on frozen `f66016e0` + ONE bounded Sol re-look on the
repair delta.** Implementor caps: soft 150 / hard 250 changed lines
(`git diff --numstat f66016e0..HEAD`), production confined to `backend.rs`.

If this round does not converge, T2 goes to **option 2** — re-scope E1/E2/E3 as
their own designed sub-slice. There is no third extension.

## Gates before fold

fmt + workspace clippy `-D warnings` + the full workspace suite on the host at
the exact final head, run unloaded. Known flake classes ledgered by name if
hit — including the whole-bin parallel class observed this round
(`cli_tests::guarded_spawn_ignores_retargeted_static_cwd_for_native_mcp`,
outside T2's diff, passes in isolation on the same tree).

Operator red/green controls run on the host at the exact head, since container
reds have been egress-blocked for several rounds.

## Custody

Bench worktree `.claude/worktrees/t2ext` at `f66016e0` for host verification.
Harness fix + diagnostic probe preserved as a patch in the session scratchpad.
Clone lives under `~/code/.a2a-implement` (reaper-covered). Both prior T2 heads
remain pushed: `435257ce` (`feat/r2f1b-3d-t2`), `f66016e0`
(`salvage/r2f1b-3d-t2-extension-candidate`).

## Execution log

- Dispatch 1: refused pre-clone by the task-spec gate (see above). No clone, no
  spawn, no cost.
- Dispatch 2 (`impl-91809-nf10irod`, base `f66016e0`): candidate `a84c8b57`,
  107 changed lines, single file — inside the 150 soft cap.
  **R1 delivered verbatim** (all three `create_dir_all`). **R2 production fix
  delivered exactly as specified**: claims the terminal phase first and returns
  doing nothing when the claim fails, `Arc::ptr_eq`-guarded removal, no durable
  record, exit guard left disarmed. **T-A** exceeds the spec — it recreates the
  root and proves the retry actually succeeds (`add_count == 1`), not merely
  that admission stops refusing. **T-C** delivered.
- **RUN WEDGED — verify hung ~3 h.** The `bridge_worktree` test binary sat at
  **0.00 % CPU** in the toolchain container. Reproduced on the host and
  root-caused to mechanism, not guessed:
  - `transferred_owner_survives_failing_control_root_pin` (T-B) calls
    `std::fs::remove_dir(&cfg.root)`. That call is **non-recursive** and the
    control root is populated by then, so it returns
    `ENOTEMPTY` (`code: 66, DirectoryNotEmpty`) — instrumented markers put the
    panic at `backend.rs:12292`, before `release_control_root_pin()`.
  - The panic unwinds, `Runtime` is dropped, and `BlockingPool::shutdown` waits
    forever on the pin-hook thread parked in
    `block_control_root_pin_for_test`'s condvar — only the never-reached
    release could free it. Sampled stacks confirm all three parked tasks
    (pin hook, runner `pinned_root`, transfer's detached `publish_terminal`).
  - **A clean assertion failure therefore becomes an unbounded hang.** This is
    why verify burned 3 h instead of reporting a red.
  - Per-test classification on the host: **T-A PASS, T-C PASS, T-B HANG**.
- **OPERATOR COMPLETION `85658e01`** (branch
  `salvage/r2f1b-3d-t2-extension-repair2`, pushed) — mechanical, no design
  content, per the 3a/3c1 repair-tail precedent:
  1. T-B `remove_dir` → `remove_dir_all`, mechanism recorded at the call site.
  2. **Restored `stalled_control_root_pin_is_observable_before_terminalization`**,
     which the agent deleted by converting it into T-B. That deletion dropped
     E2's positive proof — the stall-then-SUCCEEDS path asserting the durable
     record reaches `Some("transferred")`. T-B's pin fails, so it asserts
     `None`; it cannot cover that path. Fixture/session strings disambiguated
     so the two cannot share a temp dir.
  - **Red-first control on the restoration** (host, run-verified): mutating
    `publish_terminal` to skip the durable write fails **only** the restored
    test — `left: Some("open")` vs `right: Some("transferred")` — while the
    other four control-root tests still pass. The restored coverage is real and
    non-redundant.
  - Operator delta: 70 changed lines on top of `a84c8b57`; combined round delta
    stays inside the 250 hard cap.

- **GATES GREEN on exact `85658e01`** (host, unloaded):
  `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
  -- -D warnings` clean; full workspace suite
  **4,139 passed / 0 failed / 13 ignored across 90 targets**
  (`--no-fail-fast`, exit 0). Progression: `435257ce` 4,131/0/13 →
  `f66016e0` 4,133/**3**/13 → `85658e01` **4,139/0/13**. Count reconciles
  exactly: 4,136 tests at `f66016e0`, agent converted the stall test into T-B
  (net 0) and added T-A + T-C (+2), operator restored the stall test (+1) =
  4,139. The whole-bin parallel flake did not recur.
- **Review scope**: the last REVIEWED state is `435257ce` (the re-look that
  produced E1/E2/E3). `f66016e0` was parked before any review, so the counted
  re-look must cover `435257ce..85658e01` — 900 changed lines across
  `backend.rs`, `fs_custody.rs`, and the handoff — not merely the repair delta.

## COUNTED RE-LOOK RESULT — REJECT, 2 WRONG / 1 SMELL-DEFER

Verbatim: `reviews/2026-08-16-r2f1b-3d-t2-sol-relook2.md` (sol/max, scope
`435257ce..85658e01`, 900 lines). LSP nav was unavailable (ambiguous repo root;
`run-workflow` has no `--lang` flag), so the lens used read-only git + search —
the same limitation as the prior re-look, but worth weighting on a delta this
size.

**All three carried blockers are CLOSED:**

- **E1 FIXED** — transfer, failure, and barrier publication share one sticky
  phase CAS; root-failure, runner-exit, custody-error and initial-error writers
  all participate.
- **E2 FIXED for the original blocker** — the exact owner enters
  `preparation_flights` before the pin claim and the detached blocking open.
- **E3 FIXED for the complete in-repository writer inventory** — the
  exact-child lease reopens the name, verifies identical file identity, and
  holds the lease across the transition check, so competing protocol writers
  refuse rather than clobber.

**Two NEW WRONG blockers, both verified at source by the operator:**

1. **Root alias divergence (production-reachable now).** The backend builds the
   shared control root from the raw configured spelling
   (`backend.rs:2194`, `PathBuf::from(&cfg.root)`) while bound validation
   canonicalizes it (`provider_path.rs:102`). With a symlinked worktree root
   retargeted between admission and the lazy pin, the journal publishes `Open`
   / `Settled` under one root while custody, provider materialization, and the
   served map use the frozen canonical target under another. Transfer arming is
   irrelevant — this is reachable today. Fix per the lens: bind the shared pin
   to the frozen canonical root, carry that identity through
   `PreparationFlightJournalV1`, refuse before first publication on mismatch.
2. **Result published before reservation release.** The root-error arm calls
   `complete_with_result` (`backend.rs:3782`), which sends the caller's result
   (`backend.rs:510`), and only then performs the `Arc::ptr_eq` removal
   (`backend.rs:3785`). On the production multi-thread runtime the caller can
   resume, retry the same session, and hit the still-present `contains_key`
   check — receiving `AgentOverloaded` after a failed configure that produced
   no record and no effect. **This defect originates in the operator's repair
   spec**, which prescribed "complete the caller … then remove the entry"; the
   agent implemented exactly that. Correct order is remove-then-publish. The
   `#[tokio::test]` current-thread runtime cannot expose it.

**SMELL — DEFER (lens's ruling, operator concurs):** the converted T-B is a
false-positive — it waits until recovery is registered and configure returned
before failing the pin, so it would also pass on pre-`a84c8b57` code. And
`preparation_transfer_and_failure_claims_have_one_winner_in_both_orders` loops
over `_failure_source` without executing any caller-departure, custody-error,
or runner-exit path — it exercises the CAS helper, not those writers. This
independently corroborates the operator's finding that T-B was a converted
test rather than new coverage.

## Convergence classification at the declared cap

The cap (one repair + one bounded re-look) is now CONSUMED, and the result is
REJECT. Classifying before acting, per the discipline:

- **Trend is converging on findings count and on the carried population**:
  4 blockers (closure) → 3 (re-look) → 2 (this round), with W1/W2/W4/s2 fixed
  and sustained, and now E1/E2/E3 all fixed. Nothing previously fixed regressed.
- **But finding 1 is the THIRD distinct defect in control-root/journal-root
  handling** (W3 → E2 → root-alias binding). For that sub-area specifically,
  each round has surfaced a new instance of the same kind — the open-class
  signature. Its fix is an ownership/API change (bind and carry a root
  identity), which is precisely the "E2 ownership ripple" argument originally
  raised for option 2.
- **Finding 2 is closed and trivial** — a three-line reordering, and the
  operator's own spec error, not an artifact defect.

**Operator recommendation to the owner: split.** Take finding 2 as a mechanical
fix, and escalate finding 1 to design as its own sub-slice. No third extension
is taken unilaterally — per the declared boundary, this goes to the owner.

## Split executed (owner-approved)

**Finding 2 — FIXED, `582e832b`** (operator, red-first). The root-pin failure
arm now does `complete` → guarded `Arc::ptr_eq` removal → `send_result`, with
the removal scoped so the std mutex guard drops before the await.

- Red-first control: the new test
  `failing_control_root_pin_releases_before_publishing_its_result` was written
  BEFORE the fix and failed on unmodified `85658e01` at exactly
  `the reservation must already be released when the caller observes its result`.
  It drives the existing `pause_after_result_publication` hook, so the window
  is **deterministic** rather than a timing race — which is precisely why the
  earlier T-A/T-C could not catch it: a `#[tokio::test]` current-thread runtime
  cannot expose the race by parallelism alone.
- Gates on exact `582e832b` (host, unloaded): fmt clean; workspace clippy
  `-D warnings` clean; full suite **4,140 passed / 0 failed / 13 ignored across
  90 targets**.
- Attribution recorded plainly: this defect came from the operator's repair
  spec prescribing complete-then-remove. The agent implemented what it was told.

**Finding 1 — ESCALATED to a design sub-slice**, spec at
`plans/2026-08-16-r2f1b-3d-t2-root-identity-subslice.md`. It is the third
distinct control-root/journal-root defect in three rounds (W3 → E2 →
root-alias binding) — the open-class signature — and its fix binds and carries
a root identity through `PreparationFlightJournalV1`. Not folded into a
consumed cap.

**Reachability of finding 1 — RESOLVED, not left open.** The lens called it
"production-reachable now" but could not observe deployment state. Against the
tree: `materialize_under_custody` runs only for an admitted
`BoundWorktreeCustodyV1` (else `backend.rs:3686` returns `Legacy` before any
flight is claimed); that requires a `FrozenR2f1bContractV1`; and the only
constructor of one is `execution_policy.rs:2621`, inside the `#[cfg(test)]`
module opening at `execution_policy.rs:2580`. The defect is **latent** — it
activates when the V3 path becomes reachable. So the sub-slice gates the
V3-arming slice, not T2's landing. The lens's real contribution here stands:
the defect does not depend on transfer arming.

## Landing — PR #54, and the Windows lane (recurrence #4 of a known class)

Squash merge chosen over rebase: the branch carries three rejected intermediate
states (`c5d9390c`, `435257ce`, `f66016e0`) and `a84c8b57`'s subject begins
with a stray ``` fence (the agent copied the spec's fenced Commit Message block
verbatim). A squash lands one coherent commit and authors the message fresh,
which disposes of both problems.

`cla-assistant` passed first try — `main`'s allowlist entry `b986108c` from the
3c2 landing covered the `a2a-implement` authorship on `a84c8b57`.

**Windows CI failed, exactly as this lane's history predicts.** `pub mod
liveness` is `#[cfg(unix)]` in `lib.rs` while `fs_custody` is not, and the new
`with_existing_regular_child_lease` calls
`crate::liveness::acquire_persistent_lock_file` unconditionally. `bridge-store`
depends on `bridge-core`, so the Windows job compiles it. This is the FOURTH
instance of the class (3a, 3b1, 3c1, the 3c2 landing guard `790b4191`) — the
fs_custody/liveness cfg boundary catches every slice that adds a flock-backed
accessor, and no gate catches it before CI.

Fixed in `bf17005a` with the established 4-line `#[cfg(unix)]` guard.

**Red→green control (host, local windows-msvc probe):**

- Rebuilt the signature-only `ring` stub via `[patch.crates-io]` — ring's C
  build script cannot cross-compile from macOS, and the prior stub was
  ephemeral scratch. Probe-only; never committed. `Cargo.toml` and `Cargo.lock`
  both restored afterwards, leaving only the 4-line guard in the tree.
- Making the probe CLEAN took three iterations (dependent-requested feature
  names, `ring::hmac`, `Copy` on the hmac algorithm). This mattered: an unclean
  probe mixes stub artifacts with the real defect and cannot serve as a control.
- **RED**: exactly one error —
  `E0433: could not find liveness in the crate root` at `fs_custody.rs:717`.
- **GREEN** under `-D warnings` after the guard, with **no** second-order
  dead-code population (unlike `790b4191`, which had to chase a masked one).
- Unix unchanged: fmt clean, workspace clippy `-D warnings` clean, suite
  **4,140/0/13 across 90** — byte-identical totals before and after the guard,
  confirming it is a no-op on unix.

## Ledger items raised this round

- **Test-harness hang amplifier (SMELL, recommend fixing next round).** Any
  panic in a pin-hook-driven test before `release_control_root_pin()` converts
  a clean failure into an unbounded process hang, because the hook's condvar
  wait is unbounded and runtime drop joins the blocking pool. A bounded
  `wait_timeout` in the `#[cfg(test)]` hook would convert every future instance
  into a bounded red. Deliberately NOT folded into this round: it is not needed
  for artifact correctness, it widens the delta the re-look must review, and
  the re-look may have a view on the right shape.
- **Commit-message hygiene**: the agent copied the spec's fenced commit message
  literally, so `a84c8b57`'s subject begins with a ``` fence. Reword at landing.
- **Verify has no effective timeout** for a hung test binary — 3 h elapsed with
  no bound. Ops item.
- **No pre-CI gate for the non-unix lane (RECOMMEND FIXING — 4th recurrence).**
  Every local gate (`fmt`, `clippy --workspace --all-targets`, the 90-target
  suite) runs unix-only, so a `cfg(unix)` boundary violation in `bridge-core` is
  structurally invisible until Windows CI. It has now cost a landing round in
  3a, 3b1, 3c1, 3c2 and 3d-T2. Cheapest durable fix: add
  `cargo check -p bridge-core --target x86_64-pc-windows-msvc` to the local
  verify triad, which needs the committed `ring` stub problem solved once —
  either a vendored signature-only stub behind a probe-only feature, or
  swapping bridge-core's `ring` usage for a dependency that cross-compiles.
  Until then, every slice touching `fs_custody`/`liveness` should run the
  scratchpad probe before opening its PR.
