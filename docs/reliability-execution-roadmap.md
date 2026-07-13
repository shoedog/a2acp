# Bridge reliability execution and handoff roadmap

- **Program status:** active P0
- **R2b1 implementation baseline:** `main` at `7b788c1f` on 2026-07-12
- **Completed through:** R2b2c production-owner/workflow authority, review-approved and exact-tree full-gated (commit pending)
- **Next action:** commit and push R2b2c as the third internal R2b2 commit, then begin concurrency-qualified R2b2d
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
| R2b2 — ACP/Fable lifecycle diagnostics | **IN PROGRESS** (2a `4ed12f1`; 2b `f40096df`; 2c review 7 `APPROVE` and exact-tree gates green; 2d next) | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Observer/registry, ACP evidence, owner threading, then concurrency-qualified warm cleanup; one final merge boundary. |
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
  `agent/reliability-r2b2-acp-lifecycle`. R2b2c has final review 7 `APPROVE` and exact-tree full gates;
  commit/push 2c, then begin 2d.
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
  `f40096dfcfb43a37236ce5626fd362a16645f0fe`. R2b2c owner/workflow authority has final review 7
  `APPROVE` and exact-tree full gates on that base; its internal commit/push is next, followed by 2d.
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
