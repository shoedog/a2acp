# Bridge reliability execution and handoff roadmap

- **Program status:** active P0
- **R2b1 implementation baseline:** `main` at `7b788c1f` on 2026-07-12
- **Completed through:** R2b2d warm expiry/cleanup ownership (`14402f8`) plus final-review fold `a459b31`
- **Current exact gate:** **1,096 / 0 / 0 ignored** across the six affected packages; format/diff,
  workspace/all-target check, warnings-denied Clippy, and release build clean
- **Full workspace gate:** host serial **1,812 / 0 / 12 ignored**; repository hygiene **37** tracked
  artifacts / **7** validated example configs
- **Next action:** run one fresh full-R2b2 re-review of the `a459b31` fold before merge
- **Design of record:**
  [`superpowers/specs/2026-07-11-bridge-reliability-r2-design.md`](superpowers/specs/2026-07-11-bridge-reliability-r2-design.md)
- **Operating runbook:**
  [`../skills/a2a-bridge-operator/SKILL.md`](../skills/a2a-bridge-operator/SKILL.md)

This file is the durable program cursor. A new session should be able to start here, find the exact
active slice, open its implementation plan, and continue without reconstructing the July 2026 incident
history. The detailed design remains normative for R2; this roadmap owns sequencing, status, handoff,
and completion evidence.

## Dependency graph

```text
R2a provenance (MERGED)
  -> R2b0 contract clarifications (MERGED)
  -> R2b1 diagnostic types + rollback-safe persistence surface (MERGED)
  -> R2b2 ACP/Fable lifecycle evidence + no-replay/warm-session safety (IN PROGRESS)
  -> R2b3 API/provider mapping + remaining container/dispatch observation
  -> R2c explicit one-turn billable smoke
       -> R2d local non-billable fallback plan
       -> R3 compatibility manifest + pinned/floating canaries
            -> R4 reproducible dependency/image pins + release promotion gate

R2e authenticated in-process fallback is DEFERRED and off the critical path.
It requires R2d plus a separately approved authenticated-policy/attestation design.
```

M4 Slice 3b/3c remains parked until the reliability exit gates in
[`roadmap.md`](roadmap.md) are satisfied. Do not mix retention work into these slices.

## Program status table

| Slice | Status | Durable plan | Merge boundary |
|---|---|---|---|
| R0 — front door/baseline | **MERGED** | [`bridge-reliability.md`](bridge-reliability.md) | Docs index, compatibility matrix, priority reset. |
| R1 — Fable isolation | **MERGED** | [R1 disposition](superpowers/2026-07-11-fable-r1-disposition.md) | Host and reader controls dispositioned. |
| R2a — doctor provenance | **MERGED** at `24aff09c` | [R2 design](superpowers/specs/2026-07-11-bridge-reliability-r2-design.md) | Additive non-billable provenance rows. |
| R2b0 — contract clarifications | **MERGED** at `11ebc402` | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Design v13 retains a claim-identified expiring tombstone through cleanup and makes worktree release/forced retirement join one per-session cell; Sol/xhigh APPROVED. |
| R2b1 — diagnostic foundation | **MERGED** at `7b788c1f` | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Validated types and rollback-safe persistence/projection compatibility; no production failure-site migration. |
| R2b2 — ACP/Fable lifecycle diagnostics | **IN PROGRESS** (2a `4ed12f1`; 2b `f40096df`; 2c `40790720`; 2d `14402f8`, closure review 12 `APPROVE`; final review 1 `REVISE`, fold `a459b31`; exact **1,096 / 0 / 0**; full host workspace **1,812 / 0 / 12 ignored**; hygiene **37/7**; re-review pending) | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Observer/registry, ACP evidence, owner threading, concurrency-qualified warm cleanup, then aggregate cold-path closure; one final merge boundary. |
| R2b3 — API/container diagnostics | **NOT STARTED** | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Independently reviewed implementation after R2b2. |
| R2c — live smoke | **NOT STARTED** | [R2c implementation plan](superpowers/plans/2026-07-11-r2c-live-smoke.md) | One explicit, bounded, billable turn; no retry. |
| R2d — fallback plan | **NOT STARTED** | [R2d implementation plan](superpowers/plans/2026-07-11-r2d-local-fallback-plan.md) | Local recommendation only; never executes fallback. |
| R2e — in-process fallback | **DEFERRED / BLOCKED BY POLICY** | [R2e gated plan](superpowers/plans/2026-07-11-r2e-policy-authorized-fallback.md) | No implementation until authenticated attestation design is approved. |
| R2f — phase-aware liveness/takeover | **DEFERRED** (incident recorded) | [R2f implementation plan](superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md) | Instrument first; phase-aware stagnation, exact process-tree termination, preserved-work takeover. Starts after R2b. |
| R3 — compatibility canaries | **NOT STARTED** | [R3 implementation plan](superpowers/plans/2026-07-11-r3-compatibility-canaries.md) | Local manifest/runner first; scheduling requires runner/credential owner. |
| R4 — reproducible release policy | **NOT STARTED** | [R4 implementation plan](superpowers/plans/2026-07-11-r4-reproducible-release-policy.md) | Full resolution pins, candidate smokes, promotion and rollback. |

R2b2 executes on one merge branch in four durable internal commits: **2a** observer/storage/registry
compatibility, **2b** ACP lifecycle and safe evidence, **2c** production-owner/workflow authority, then
the concurrency-qualified **2d** warm expiry and cleanup single-flight. The [R2b implementation
plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) is the restart contract for each item.

### Deferred incident: verification phase parked after useful edits

`INC-VERIFY-STALL-2026-07-11` records an operator-reported Luna run in `~/code/stockTrading`: 2h54m total,
useful edits completed in about 25 minutes, last file edit at 17:22, then nearly three hours parked in
verification. The operator terminated only that process tree, preserved the work, completed verification
manually, and found the changes clean. Root cause is **unknown**; the evidence does not yet separate a
provider/adapter stall, child-process failure, or orchestration waiter leak.

The deferred [R2f plan](superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md) requires phase-aware
meaningful-progress evidence, false-positive controls for silent long tests, a stagnation snapshot, exact
process-tree identity, and a bounded takeover artifact. Do not use file mtime or process existence alone,
do not broad-kill by process name, and do not auto-start a duplicate billable attempt.

Allowed status values are `NOT STARTED`, `IN PROGRESS`, `IN REVIEW`, `MERGED`, `BLOCKED`, and
`DEFERRED`. Update this table in the same PR that changes a slice status. Never mark `MERGED` from a
local commit or open PR.

## Resume protocol for a new session

1. Fetch and verify the live default branch:

   ```bash
   git fetch origin main
   git rev-parse origin/main
   git status --short --branch
   ```

2. Read, in order:

   - [`docs/README.md`](README.md)
   - this roadmap
   - the active slice plan from the status table
   - the R2 design of record for R2b–R2e, or the R3/R4 plan for later work
   - [`compatibility.md`](compatibility.md) before any live agent or release claim
   - the operator skill before any bridge-mediated review or smoke

3. Confirm that every prerequisite slice is actually present on `origin/main`; do not trust this table
   if live history disagrees.

4. Create an isolated worktree from current `origin/main`. For the next slice:

   ```bash
   git worktree add /private/tmp/a2a-bridge-r2b0 \
     -b agent/reliability-r2b0-contract origin/main
   ```

5. Before editing, run the active plan's named pre-change regression. Every new behavior needs a test
   that fails on the pre-change code plus a negative or edge case.

6. Keep scratch configs, prompts, model output, SQLite stores, and review artifacts under
   `/private/tmp`; repository hygiene forbids committing one-off workflow material.

7. Before merge, update this roadmap with completion evidence and the next single action.

## Universal slice rules

### Safety and compatibility

- The public A2A error category remains redacted. Diagnostic detail is operator evidence, never an
  untrusted wire payload.
- No prompt may be replayed after the conservative prompt-acceptance barrier.
- Provider-capacity handling and container degradation are separate. Neither automatically routes to
  another provider or execution tier.
- Tier 2 remains mandatory for untrusted reads; Tier 3 remains mandatory for all write-capable work.
- Caller metadata, workflow input, config labels, and `AlwaysGrant` cannot assert trusted-content
  authority.
- Raw advertised capabilities win. Aliases and semver ranges are not provenance.
- Raw opaque stderr is never persisted or traced. Best-effort-redacted stderr remains explicit opt-in.

### Implementation discipline

- One numbered sub-slice per branch/PR unless the active plan explicitly permits coalescing.
- Use additive defaults at public traits and serialized records whenever rollback compatibility is part
  of the contract.
- Match every new enum exhaustively in retry, warm-session, projection, metrics, and serialization
  code. Wildcard arms must not hide a new failure class or event variant.
- Run the repository's complete suite, not only focused tests. Report exact passed/failed/ignored totals
  and every unexercised live gate.
- A failure outside slice scope is reported, not re-baselined or silently fixed.

### Required merge gates

Every implementation slice runs:

```bash
git diff --check
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --bin a2a-bridge
cargo run -p a2a-bridge -- validate --repo-hygiene
```

Docs-only R2b0 still runs repository hygiene and the full workspace suite because it changes the design
contract that later code will follow.

### Review and dogfood gate

- R2b–R2e require a fresh adversarial full-branch review through the bridge before merge.
- Prefer Fable/xhigh when its usage window has headroom. If it is degraded or near its limit, an operator
  may select `gpt-5.6-sol`/`xhigh` as a new, separately recorded attempt. Never auto-resume or auto-route
  the first attempt.
- `max` prioritizes depth rather than parallelism and is reserved for tightly connected evidence: complex
  memory leaks, deadlocks/data races or other concurrency failures, transaction-safety proofs, critical
  algorithm correctness, zero-downtime migrations, rare production failures, or a problem that
  High/xhigh failed to resolve. Record that reason before launch and budget the watchdog for a run that
  may exceed one hour; ordinary full-branch/spec reviews use xhigh.
- Every finding is tagged `WRONG` or `SMELL`; a `WRONG` finding names the constructible state and
  incorrect result. Prior findings are adjudicated before new findings.
- R3/R4 additionally require a release/compatibility reviewer focused on credentials, cost bounds,
  workflow permissions, immutable pins, and rollback.

## Completion evidence template

Fold this block into the active plan and this roadmap when a slice merges:

```text
Status: MERGED
Branch:
Commit:
PR or direct-main record:
Review model/effort and verdict:
Focused regressions:
Full suite totals:
Build/hygiene gates:
Live/billable gates run:
Live/billable gates not run:
Compatibility rows changed:
Remaining findings/deferred items:
Next action:
```

## Current handoff

- `origin/main` contains R2b1 at `7b788c1f`; R2b2 is active on
  `agent/reliability-r2b2-acp-lifecycle`. R2b2d is review-approved, committed, and pushed at
  `14402f895a5eda2852684a8fbd35f83452e2645f`; the final full-branch review fold is committed at
  `a459b31de5a4665138a7330868e38dfb8992438b`, with fresh re-review pending.
- R2b0's full local suite was 1,607 passed / 0 failed / 12 ignored live-agent tests.
- A fresh bridge-mediated Fable/xhigh review returned `R2A: READY`, `V6 DESIGN: READY`, `MERGE`.
- The Podman bare image-id normalization and non-vacuous descendant survivor-marker regression were
  folded after that review and revalidated by the full local gate set.
- R2b1 foundation code is additive; no production failure site constructs `AgentFailure` yet.
- R2b0 design v13 landed from `agent/reliability-r2b0-contract`. The first Sol/max review returned `REVISE`:
  cold resolution preceded the named owners, direct correlation ids lacked durable task rows, diagnostic
  observation collided with the existing rich-event method, two warm-reconcile debug sinks were omitted,
  and the plan named one nonexistent helper. V8 folded those findings. Its re-review closed five, left
  direct-workflow journal authority partial, and found agent-controlled success-trace leakage plus an
  unsafe cached/teardown observer lifetime. V9 folded those three. Its max review closed storage authority
  and trace coverage, then required concrete spawn/reap/worktree seams, transition-wide credential
  redaction, and shared warm-session failure retirement. V10 folded those items. Its Sol/xhigh review
  closed three findings and found one cancellation window around async expiry. V11 folds a synchronous
  drop-action plus owned expiry-claim handoff. Its Sol/xhigh review closed the pre-claim race but found
  cleanup could be canceled/restarted after its first side effect. V12 transfers resources into one
  observer-free cleanup flight before the first await. Its concurrency-qualified max review found that
  early handle removal still exposed deterministic `g0` remint and forced worktree retirement could race
  release. V13 retains a non-reusable tombstone until the exact flight succeeds and makes release/retire
  join one worktree cleanup cell. A fresh Sol/xhigh re-review adjudicated both `FIXED`, found no new issues,
  and returned `APPROVE`.
- R2b0 local gates passed: Markdown links, `git diff --check`, fmt, workspace check, clippy with warnings
  denied, **1,607 passed / 0 failed / 12 ignored**, release binary build, and repository hygiene (37
  tracked artifacts / 7 example configs). The approved contract was fast-forwarded to `origin/main` at
  `11ebc402`.
- R2b1 is implemented on `agent/reliability-r2b1-diagnostic-foundation`: private validated diagnostic
  DTOs, static `AgentFailure` formatting with typed retry behavior, optional rollback-compatible progress
  diagnostics, total live/snapshot projection, and Memory/SQLite journal coverage. No production failure
  site constructs `AgentFailure`; the source guard enforces that boundary.
- R2b1's first bridge-mediated Sol/xhigh review returned `REVISE`: dynamic diagnostic progress text,
  mixed-case URL queries, unbounded reset timestamps, and contradictory stderr counts were constructible;
  live/reattach path coverage, exact mapping assertions, and the source guard were too weak. All seven
  are folded with focused regressions; clippy is clean with warnings denied; the full workspace suite
  passed **1,629 / 0 / 12 ignored**; release build and repository hygiene passed (37 tracked artifacts /
  7 example configs). The first closure re-review marked six `FIXED`, the AST guard `PARTIAL`, and found
  two new `WRONG` invariant gaps: post-barrier container fallback and retryable fatal classes. Those three
  are folded with an exact class/phase/barrier matrix and alias/cfg-aware guard regressions. Clippy is
  clean with warnings denied and the full workspace suite passed **1,630 / 0 / 12 ignored**. Rebuild the
  release binary and hygiene passed. The third fresh review marked both typed invariants `FIXED`, kept the
  guard `PARTIAL`, and found failed-clock reset validation could accept an unbounded timestamp. The guard
  now scans and counts the exact central `error.rs` builder, and reset metadata rejects a missing/invalid
  reference clock. The fourth fresh review marked both `FIXED`, found no new `WRONG`, and requested direct
  negative/overflow reference-time tests; those now cover reset-bearing rejection and reset-free
  acceptance. The exact final code-and-test tree passed fmt, clippy with warnings denied, and the full
  workspace suite at **1,630 passed / 0 failed / 12 ignored**. The release build and repository hygiene
  gate also passed (37 tracked artifacts / 7 example configs); production code was unchanged after those
  two gates. The final bounded Sol/xhigh test-closure review marked the last `SMELL` `FIXED`, found no new
  `WRONG` or `SMELL` findings across the named closed surfaces, and returned `APPROVE`. R2b1 is merge-ready.
- The 12 ignored tests are authenticated real-agent/two-bridge and local Ollama coverage; no live R2c
  billable smoke was run in R2b1.
- R2b1 was fast-forwarded to `origin/main` at `7b788c1fa6b62459e8a8473ca853f9414b28bfbc` after the
  final `APPROVE`; the post-merge cursor branch is `agent/reliability-r2b2-cursor`.
- R2b2 implementation is active on `agent/reliability-r2b2-acp-lifecycle`; R2b2a is committed at
  `4ed12f1035c16fa5dbd55169e59ca4c277373da4` and R2b2b at
  `f40096dfcfb43a37236ce5626fd362a16645f0fe`. R2b2c owner/workflow authority is committed and pushed at
  `407907202982d732c2395be0f6319f6029622f82` after final review 7 `APPROVE` and exact-tree full gates;
  R2b2d is approved/pushed at `14402f895a5eda2852684a8fbd35f83452e2645f`, and the aggregate final-review
  fold is committed at `a459b31de5a4665138a7330868e38dfb8992438b` with re-review pending.
- R2b2a adds bounded/no-op/task-journal diagnostic observers and explicit factories, composite backend
  compatibility methods, `resolve_observed`, legacy/observed registry spawn constructors, initializer-only
  observer ownership, cache/waiter `backend.reused`, and live `new_observed` wiring. No ACP lifecycle
  failure site is migrated yet. The first bridge-mediated Sol/xhigh review returned `REVISE` for one
  `WRONG/MAJOR` (journal grammar advanced before an awaited write committed) and one `SMELL/MINOR`
  (observer `Debug` secrecy lacked a secret-bearing regression). The fold stages grammar on a clone,
  commits only after successful persistence while serializing the observer, and adds deterministic write
  error/cancellation plus exact `Debug` regressions. The fresh closure review adjudicated both `FIXED`,
  found no new `WRONG` or `SMELL`, and returned `APPROVE`.
- R2b2a's exact post-fold tree passed workspace check, warnings-denied all-target clippy, **1,640 passed /
  0 failed / 12 ignored**, release binary build, and repository hygiene (37 tracked artifacts / 7 example
  configs). The ignored set remains authenticated real-agent/two-bridge and local Ollama coverage.
- R2b2b threads structured lifecycle observation through ACP spawn/initialize/auth/session/config/prompt
  and operation-owned teardown; adds accepted-work no-replay fencing, bounded process-scoped redacted
  stderr, deterministic cancellation settling, and an AST-enforced typed trace funnel. Its first full
  review plus ten closure reviews are recorded in the implementation plan. The latest fold makes process
  stderr metadata-only monotonically after an uncertain credential-bearing mint or session removal, so a
  later finite redactor replacement cannot expose delayed text. Prior folds keep accepted work evidence
  through route removal, own active mint-cwd cleanup before redactor awaits, order cancel delivery against
  prompt installation, and preserve effective-cwd values across live sessions. Deferred R2f evidence
  remains on its own parked branch. Focused gates pass: bridge-acp **183 / 0**,
  bridge-container **24 / 0**,
  R2b1 diagnostics **20 / 0**, process lifecycle **13 / 0**, targeted host/core MCP regressions, and
  warnings-denied changed-crate Clippy. The fresh Sol/xhigh closure review returned `APPROVE` with no new
  `WRONG` or `SMELL`. The exact code tree passes workspace check, workspace/all-target warnings-denied
  Clippy, **1,700 passed / 0 failed / 12 ignored** across 46 test executables, release build, format/diff,
  and repository hygiene (**37** tracked artifacts / **7** example configs). R2b2b is committed and pushed
  at `f40096df`; do not merge the R2b2 branch until R2b2a-d plus the full-branch review gate are complete.
- R2b2c threads one explicit operation observer through direct inbound streaming, synchronous, and fan-out
  owners; coordinator prompt/continue; fresh and child warm session checkout; cold/retry/warm workflow
  execution; the implement `TurnRunner`; and the worktree decorator. The additive
  `WorkflowDiagnosticContext` wrapper owns the explicit per-node/attempt factory without changing the
  exhaustively constructible public `WorkflowRunContext`. Direct and correlation-only workflows remain
  bounded in-memory even when they carry a `task_id`; detached owners install the journal factory only
  after proving the durable task row exists. Mutation-sensitive tests require exact observer identity across resolve/checkout/prompt,
  preserve one rich event with one flush, allocate a fresh observer per retry attempt, reject a missing
  durable row before prompt, and make a journal diagnostic write failure fail the task before backend
  prompt. The complete affected-crate run passed **912 tests** except three unrelated process-fixture
  precondition failures under parallel execution; all **13 process tests** passed immediately in isolated
  serial execution. Its first fresh bridge-mediated Sol/xhigh review inspected all 12 then-changed paths,
  found no untracked files, and returned `REVISE`: one `WRONG/MAJOR` showed a warm prompt-open future could
  record a rich event and then lose it when cancellation won before stream return; one `SMELL/MAJOR` showed
  the two production implement callsites were not mutation-locked against a return to legacy `run_turn`.
  The fold flushes the constructed sink exactly once before canceled completion and routes edit/fix through
  one observed-only helper whose test panics on legacy dispatch. Self-audit also routes the non-task ACP
  catalog probe through one in-memory observer across spawn and discovery, emits structured discovery
  session-create failures, prevents ACP traffic when observation fails, and preserves a primary canceled
  `Done` outcome when rich flush also fails. The four catalog tests, both warm flush/cancellation tests, and
  the observed implement helper test pass; workspace check and workspace/all-target Clippy are clean with
  warnings denied. Closure review 2 adjudicated both inherited findings `FIXED`, independently verified both
  self-audit folds, and then returned `REVISE` for the analogous `WRONG/MAJOR` cold prompt-open race: its
  sink was constructed inside the cancel-raced future, so a recorded rich event could be dropped without a
  flush. The second fold hoists cold sink ownership outside the race, flushes once before canceled cleanup,
  and adds a deterministic record-then-cancel regression with exact event/flush counts. That regression
  passes. Closure review 3 marked all three inherited findings `FIXED` and both self-audit folds verified,
  then returned `REVISE` for two `WRONG/MAJOR`, one `SMELL/MAJOR`, and one `WRONG/MINOR`: the required public
  context field broke downstream exhaustive literals; detached checkpoint failure could drop an in-flight
  sibling rich event; warm inbound/catalog owner seams lacked mutation locks; and this status row was stale.
  The fold moves authority into additive diagnostic-context entrypoints with an external exhaustive-literal
  compile regression; cancels and drains detached siblings after the first sink error while retaining that
  primary error; proves a real two-root detached checkpoint race flushes its pending sibling exactly once;
  and adds exact observer-identity tests for warm unary/streaming and the injected production catalog owner.
  Focused workflow, detached, inbound, and catalog tests pass. Closure review 4 adjudicated all seven
  inherited findings `FIXED`, verified both self-audit folds, and found no new code/test defect. Its sole
  `WRONG/MINOR` was the implementation plan header's stale “2b closure review pending” cursor; the header
  then recorded 2b at `f40096df`, 2c's review-4 fold, and 2d not started. Review 5 marked that header
  finding `FIXED`, found no code/test defect, and reported only one `WRONG/MINOR`: the status table
  abbreviated 2b as `f40096d` while the other cursors used `f40096df`. Review 6 marked the corrected exact
  prefix `FIXED`, found no code/test defect, and reported one `WRONG/MINOR`: the older Current handoff
  sentence still called the first 2c review “next.” Review 7 marked that inherited finding `FIXED`, read
  the complete 16-file base diff, found no new code/test defect or cursor contradiction, and returned
  `APPROVE`. The exact tree passes format/diff checks, workspace check, workspace/all-target warnings-denied
  Clippy, **1,725 passed / 0 failed / 12 ignored** across 47 test binaries in serial execution, the release
  binary build, and repository hygiene (**37** tracked artifacts / **7** example configs). This full serial
  gate also clears the three unchanged process-fixture precondition failures seen only during the earlier
  parallel affected-crate run. No live/billable gate was run. Commit/push 2c, then begin 2d; R2f
  phase-aware liveness/takeover remains deferred and does not reopen 2c absent a concrete 2c contract
  violation.
- R2b2d now fails closed for every structured warm `AgentFailure` through one exhaustive classifier and
  exact `WarmCompletionGuard`. Synchronous error observation arms expiry before any await; an exact
  generation/operation claim replaces the live handle with a claim-id tombstone, and an observer-free
  cleanup task owns backend release, lease drop, child pruning, and exact tombstone finalization. Explicit
  release, idle reap, cancel failure, and multi-child release all claim ownership before cleanup awaits;
  canceled report waiters detach from the task. Cleanup failure/panic remains a non-reusable
  `cleanup_failed` tombstone plus one bounded backend/session retry capability; release/clear reclaims it
  under a new exact claim, failure restores it, and success clears it. Stale operation/claim completion
  cannot clear newer state. Per-operation abort-token retention closes cancel-A/checkout-B/release and
  reset orphan windows.
- Worktree cleanup is sealed and per-session: synchronous cell acquisition precedes the first await;
  observer-free shared flights retain component state and reports; equal callers receive one report;
  `forget < release` upgrades survive waiter cancellation; provider/sidecar failures retry only incomplete
  components; reservation settlement precedes inner release; and retirement joins the same cell before
  `inner.retire`. Successful configure retains a bounded per-session cell, while admission, seal, and
  success eviction linearize under the cell-map lock. The cell retains exact worktree metadata until
  provider/sidecar completion. Observed callers record the shared result locally, while observer-start
  persistence failure remains fatal without canceling cleanup.
- Self-audit produced reproducible red-to-green races for child-sweep cancellation (one release instead of
  three), provider removal canceled with its waiter, configure-after-release resurrection, divergent
  concurrent cleanup reports, lost stronger upgrades, warm cancel stuck in `cancelling`, and ready backend
  failure losing to simultaneous cancellation/receiver close. One combined gate then exposed a scheduler
  regression: unconditional cancel-flight spawn let the workflow producer free its busy token before
  SessionCancel returned. The settled pattern polls the owned settlement once inline and transfers that
  same partially-polled future to a detached task only when pending; the formerly parked 50-test producer
  binary now passes in 0.07 seconds.
- The first bridge-mediated Max review returned `REVISE` with four concrete R2b2d failures and one correctly
  deferred R2f smell. Red regressions reproduced: structured workflow failure dropped during a blocked rich
  flush (`cancel`, expected `release`); release passing a configure blocked in the pre-reservation git probe;
  real git remove returning success while its target remained; and one retained successful cleanup cell per
  distinct session. The fold adds synchronous `NodeTurnCleanup::arm_exit` before prompt-open/stream flush,
  publishes per-session configure admission before every git/inner await and makes cleanup wait for it,
  propagates real remove/prune failures while keeping already-absent removal idempotent, and evicts only the
  exact successful flight before publishing its report. Failed component state remains for explicit retry;
  concurrent waiters and stronger replacement flights retain their own `Arc`/report generations. The four
  red schedules now pass, with additional no-cwd, non-git, prompt-open, stream-error, idempotent-remove, and
  failed-retry edges. The reviewer-classified inherited reset/reconcile/compact and sequential child sweep
  concern remains R2f; it does not reopen R2b2d without a current-slice failure.
- Closure review 1 marked the workflow-arm and pre-reservation-admission findings `FIXED`, but returned
  `REVISE` for two narrower worktree boundaries: absent checkout plus successful prune did not prove the
  exact Git registration was gone, and successful cell eviction let a warm release that began after
  retirement's snapshot create an unjoined second inner release. The Git fold now uses cancel-safe child
  commands and requires target metadata absence, successful prune, and exact `worktree list --porcelain -z`
  registration absence. Metadata errors, prune failure, or a retained exact record fail closed; ordinary
  removal and repeated already-absent removal remain successful. The installed Git eagerly removed the
  attempted fresh/locked registrations, so that environment-specific retained-registration schedule could
  not be executed end-to-end locally; the final-state truth table and byte-exact porcelain parser are
  deterministic, while real-Git positive and target-remains failure tests exercise the command path.
- Sealing retains known-session cells for the bounded retiring backend lifetime, so the still-live warm
  owner joins retirement's exact report; unknown late session ids cannot create cells after seal. A gated
  test first reproduced two inner releases and now proves one release before `inner.retire`. Self-audit also
  reproduced a previously hidden ownerless-`Reserving` timeout when configure was canceled during provider
  add. Reservations now carry configure-owner identity and cleanup metadata; release, a concurrent configure,
  or retirement takes over only after that owner disappears. Git subprocesses use kill-on-drop so canceled
  configure cannot leave an unowned add child racing cleanup.
- Verification accounting was corrected during the closure-1 fold: two earlier worktree crate commands had yielded
  before completion and remained parked at an acknowledgement made unreachable by the new admission wait.
  Only those exact Cargo/test pairs were terminated; the acknowledgement now marks the actual admission
  wait. The complete worktree crate then emitted its terminal summary: **38 passed / 0 failed / 0 ignored**.
- Closure review 2 returned `REVISE`. It marked the Git predicate and harness `FIXED`, configure cancellation
  `PARTIAL`, and retirement `NOT FIXED`, then demonstrated four current-slice failures: the seal-to-cell
  known-owner gap; loss of ownerless-reservation `WtEntry` on provider failure; prompt-open cancellation
  preceding a ready structured error; and unreachable `CleanupFailed` backend cleanup. Deterministic red
  tests reproduced all four. The folds retain configured cells and worktree metadata, poll prompt-open
  backend results first, and retain a minimal retry owner for release/clear. Partial provider-add, sidecar-
  write, inner-configure, repeated retry-failure, and canceled-retry-waiter edges also pass. The Git
  retained-registration command fixture remains an explicit coverage limitation; R2f remains deferred.
- Closure review 3 marked ownerless metadata, prompt-open precedence, retry capability, and docs `FIXED`,
  but returned `REVISE` with three worktree blockers. A later failed/canceled configure erased an earlier
  no-cwd configured cell; cleanup-start rejection decremented a counter it never incremented, wrapping zero
  to `u64::MAX`; and observed cleanup awaited `teardown started` persistence before selecting its flight.
  All were reproduced red. Configured ownership now persists in cell lifecycle, rejected admission leaves
  the counter unchanged, and observed cleanup starts/joins the observer-free flight before its first
  diagnostic await. The canceled/error admission, retirement-counter, and pending-observer cancellation
  schedules pass; the full worktree package passes **47 / 0 / 0**.
- Closure review 4 marked all three closure-3 blockers `FIXED`, then returned `REVISE`: outer
  `WarmCompletionGuard` claimed the session but awaited teardown-start persistence before spawning cleanup,
  and an immediate observation error returned before cleanup settled. Both schedules reproduced red.
  `ExpiryClaim::into_flight` now transfers ownership synchronously; the guard starts cleanup before its
  diagnostic await and joins it before returning observation failure.
- Closure review 5 marked the observer-ordering fold `FIXED`, then returned `REVISE`: a public lease
  destructor panic after checked backend release escaped the release-only unwind boundary. `join_flight`
  converted the resulting `JoinError` to `AgentCrashed` without the exact claim identity, leaving the
  context permanently `Expiring`. Lease-panic and explicit task-abort schedules reproduced the stuck state.
  `CleanupFlight` now retains a generation/operation/claim-bound settlement capability in both its whole-
  worker unwind recovery and its joiner; either failure installs one bounded retry owner only for the exact
  tombstone. Both former-red schedules pass, including successful explicit retry.
- Closure review 6 marked the claim-aware worker/joiner fold `FIXED` but found a separate `WRONG/BLOCKER`:
  partial Worktree configuration plus failed compensation retained exact cleanup metadata after every
  production caller dropped the only session/lease owner. A subsequent same-session configure was rejected,
  while distinct failures could accumulate cells. The exact schedule reproduced red. Failed configuration
  now marks its cell before dropping admission; the reporter owns exponential-backoff release retries in the
  same flight slot, and explicit release/retirement can replace the failed slot by flight id. While any such
  cell is pending, new allocation is rejected before provider add; a 64-admission circuit breaker bounds the
  already-in-flight wave. Recovery resumes only incomplete components and reopens admission after exact
  eviction. Review 6's test-proof smell is also folded: a worker-only lease-panic regression removes the
  joiner's settlement capability before the panic. Worktree passes **49 / 0 / 0** and coordinator passes
  **228 / 0 / 0**.
- Closure review 7 marked worker-only panic recovery `FIXED`, but returned `REVISE` for two Worktree
  blockers. Cancellation after reservation/provider side effects dropped an unmarked admission before any
  reporter existed, bypassing autonomous recovery and the capacity bound. Separately, a `Forget` caller could
  replace a completed failed `Release` slot at weaker strength, clear the marker, and stop Release recovery.
  Both schedules reproduced red. Reservation publication now arms cleanup-on-drop; admission destruction
  marks and synchronously starts the observer-free Release flight after balancing counters. Failed-slot
  replacement retains `max(existing, requested)` strength, so weaker takeover joins/retries Release. The
  cancellation schedule owns cleanup without manual release, rejects a distinct allocation, and retries only
  provider removal; the downgrade schedule performs two Releases and zero Forgets. Review 7's component-count
  proof gap is folded into the autonomous-retry test. Worktree passes **51 / 0 / 0**.
- Closure review 8 marked the standalone cancellation and completed-failure strength folds `FIXED`, but found
  one cross-flight `WRONG/BLOCKER`: a pending Forget superseded by destructor-owned Release could report
  success and clear the shared failed-config marker before checking flight identity. If Release then failed,
  admission reopened and automatic retry stopped. The combined schedule reproduced red. Success finalization
  now requires the reporter's exact current flight id; a present failed-config marker additionally requires
  current Release strength. Stale/weaker success reports only to its own waiter and preserves component
  progress. The cross-product regression covers pending Forget, canceled configuration, failed Release,
  degraded admission, and automatic Release recovery. Worktree passes **52 / 0 / 0**.
- Closure review 9 marked closure 8's reporter-identity/strength fold `FIXED`, then returned `REVISE` for one
  cross-owner `WRONG/BLOCKER`: after a structured failure was synchronously armed, concurrent
  `SessionCancel` could claim the exact running handle, settle backend cancellation to `Idle`, and clear the
  operation before the delayed expiry claim acquired the table. Both settlement orders reproduced red.
  Each warm turn now carries one opaque exact-operation expiry intent shared by its completion guard and
  retained session-table turn record. Structured failure publishes that intent synchronously; cancel
  settlement treats it as deferred expiry, and an expiry arriving while `Cancelling` sets the same deferred
  flag. An already-settled `Idle` handle may be claimed only for the exact retained armed operation; the
  existing stale-operation regression proves a newer running operation remains untouched. Coordinator passes
  **230 / 0 / 0**.
- Review 9's `SMELL/MAJOR` mutation-proof gap is also folded. A deterministic Worktree regression now creates
  the distinct state “failed-config marker present, exact current flight still Forget” and proves Forget
  success cannot clear or evict it; weakening the Release predicate to accept Forget makes that test fail at
  the marked-cell assertion. A separate marker-free Forget control proves immediate cell eviction. Worktree
  passes **54 / 0 / 0**.
- Closure review 10 marked Worktree finalization and the review-9 strength proof `FIXED`, but returned
  `REVISE` for two adjacent production schedules. First, cancel could settle A to `Idle`, A could then arm
  structured expiry, and successor checkout B could mint before A claimed the table; A became stale and the
  poisoned backend remained reusable. Both `checkout_existing_turn` and ordinary no-diff checkout reproduced
  this red. `WarmExpiryIntent` is now a three-state atomic linearization point: `open` transitions exactly once
  to `armed` or `successor_reserved`. If failure wins, checkout atomically installs one expiry claim and returns
  `SessionExpired`; if successor admission wins, a later stale guard cannot arm or release B. The two former-red
  checkout schedules plus the direct two-order atomic control pass. Coordinator passes **233 / 0 / 0**.
- Second, ready-backend priority was reapplied on every workflow/inbound drain iteration. A 128-item ready
  usage prefix deterministically delayed already-ready cancellation/disconnect through all 128 items. The next
  concrete backend item still wins once, preserving queued structured-error precedence; after any benign item,
  workflow checks cancellation and inbound usage checks receiver closure before polling data again. Inbound
  disconnect finalization is shared by closed-select, usage, and failed-send paths. The former-red burst tests
  now consume exactly one item; prior ready-error, usage/no-usage disconnect, send-error, and producer-ordering
  controls remain green. Workflow passes **76 / 0 / 0** and inbound passes **263 / 0 / 0**.
- Closure review 11 adjudicated both closure-10 production folds and every inherited implementation/test
  surface `FIXED`. Its sole `WRONG/MINOR` was incomplete authoritative summary metadata: the design header,
  roadmap top/table, and plan header did not all spell out closure 10, **1,090 / 0 / 0**, and the pending
  full-workspace/hygiene boundary. Those entrypoints now carry the exact same state. The retained Git fixture
  and bounded-yield polling remain `SMELL/MINOR`; no code/test finding remains open from review 11.
- Closure review 12 used a fresh Sol/xhigh read-only instance because review 11 had already completed the Max
  concurrency audit. It confirmed that only the three authoritative documentation files changed after review
  11, adjudicated the summary fold correct, retained the two accepted minor coverage debts, found no new
  `WRONG`, and returned `APPROVE`.
- The post-fold exact six-package gate passes **1,090 / 0 / 0 ignored**. `cargo fmt --all -- --check`,
  `git diff --check`, `cargo check --workspace --all-targets`, warnings-denied workspace/all-target Clippy,
  and the workspace release build are clean on the same tree. The first managed-sandbox full-suite attempt
  stopped in the CLI binary at **268 passed / 14 failed**: 12 Wiremock cases could not bind an OS port
  (`PermissionDenied`) and two file-watch cases timed out. The identical host-level serial command then passed
  **1,806 / 0 / 12 ignored** across 64 terminal result groups, falsifying a branch regression. Repository
  hygiene passes with **37** tracked artifacts and **7** validated example configs. The Git command-fixture
  limitation and bounded yield polling remain minor test-coverage follow-ups. No docs-link checker is present
  and no live/billable gate has run.
- The first final full-R2b2 Max review inspected all 36 changed paths at published head `5917f175` and
  returned `REVISE` with two `WRONG/MAJOR` cold-workflow asymmetries. A successful cold turn could discard a
  result-bearing Worktree teardown failure and report `Completed`; separately, cancellation-first cold
  prompt-open and stream-drain selects could discard an already-ready structured failure. Self-audit then
  proved the terminal aggregator also rewrote a correctly selected warm failure to `Canceled` whenever the
  shared cancellation token remained set.
- Fold `a459b31de5a4665138a7330868e38dfb8992438b` routes every cold cleanup through the same attempt observer
  and result-bearing forget/release methods, makes a ready concrete backend result win once before a control
  check, and carries each node's actual completed/failed/canceled disposition into workflow terminal
  aggregation. Deterministic pre-fold runs were red for cleanup false-success and both cold ready-error races;
  both warm terminal assertions were also red. Negative/edge controls preserve the earlier backend failure,
  pending-stream cancellation, one-benign-item bound, and cancel/cleanup-error visibility. Workflow passes
  **82 / 0 / 0**.
- The folded six-package gate passes **1,096 / 0 / 0 ignored**. The host serial full workspace passes
  **1,812 / 0 / 12 ignored**. Format/diff, workspace/all-target check, warnings-denied workspace/all-target
  Clippy, workspace release build, and repository hygiene (**37/7**) are clean. The ignored set is unchanged;
  no live/billable gate ran. Run one fresh full-R2b2 re-review before merge.
