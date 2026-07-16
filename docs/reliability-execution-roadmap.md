# Bridge reliability execution and handoff roadmap

- **Program status:** active P0
- **Current main base:** `origin/main` at `be54bc51` on 2026-07-15 (PR #28 merged R2c)
- **Completed through:** R2c **MERGED** at `be54bc51`; its fixed candidate passed the explicitly
  authorized artifact-exact host-Codex lane before merge
- **Active slice:** R2d **APPROVED / PENDING MERGE** on `agent/reliability-r2d-fallback-plan`
- **Current exact R2d deterministic gate:** the v23 working fold passes fallback-plan CLI **24 / 0**,
  smoke units **22 / 0**, and local-file units **7 / 0**; a Linux container also passes planner **24 / 0**,
  local-file **7 / 0**, and the guarded-composition regression **1 / 0**. The full serial workspace passes
  **1,985 / 0 / 12 ignored** across 69
  test/doc-test executables. Format/diff, all-target check, warnings-denied Clippy, release build, and
  repository hygiene **37/7** are clean
- **Review state:** the initial bridge-mediated `gpt-5.6-sol`/`xhigh` review of `b6424d7`, closure
  re-review 1 of `0b05c409`, closure re-review 2 of `c8d17b2`, closure re-review 3 of `69152d73`,
  closure re-review 4 of `349755ed`, closure re-review 5 of `4971647`, closure re-review 6 of
  `379c3ac`, and closure re-review 7 of `7fec898` all returned `REVISE`. Closure re-review 8 of
  `1586f24` adjudicated the sole review-7 ledger finding `FIXED`, found no new `WRONG` or `SMELL`, and
  returned `APPROVE`.
  Re-review 4 marked secrecy/auth items `FIXED`,
  kept directory-object identity `PARTIAL`, and found serializer-impossible cleanup evidence plus two
  documentation smells. V19 added a descriptor-derived persistent object fingerprint, exact cleanup-tuple
  eligibility, explicit status-cursor ownership, and the corrected spawn comment. V20 binds Darwin file
  identity to its volume UUID and Linux file-handle identity to the boot ID plus a non-reused 64-bit unique
  mount ID. Re-review 5 marked all four inherited items `FIXED`, then found unbound semantic source-mount
  drift plus two stale authority surfaces. V21 binds the plan-time mount object through action and aligns
  both docs. Re-review 6 marked all three v21 items `FIXED`, then found target static-cwd alias
  dereferencing after authorization plus a stale next-action clause. V22 composes every guarded
  cwd-derived input from the pinned object path before spawn and aligns the cursor. Re-review 7 marked
  both inherited findings `FIXED` and found only that the current Linux gate ledger omitted the explicit
  guarded-composition **1/0** total. V23 aligns that total across every current review-boundary surface.
  Fable and Claude are not planned under the constrained usage windows
- **Last merged full workspace gate:** R2c host serial **1,933 / 0 / 12 ignored** across 68 executables;
  workspace/all-target check, warnings-denied Clippy, release build, and repository hygiene **37/7** clean
- **Current execution boundary:** attempt 1 on `ce605eaf` passed provider/lifecycle but was rejected for an
  initial `0644` artifact; after reviewed create-new hardening, separately authorized attempt 2 on
  `1c9e4a43` passed in 8.770 seconds with a `0600` artifact, exact terminal `PONG`, no tools/retry/fallback,
  and completed teardown
- **Next action:** push `agent/reliability-r2d-fallback-plan` and open one non-draft R2d PR. After merge,
  advance the roadmap to R3; R2e remains deferred and must not start from this branch
- **Design of record:**
  [`superpowers/specs/2026-07-11-bridge-reliability-r2-design.md`](superpowers/specs/2026-07-11-bridge-reliability-r2-design.md)
- **Operating runbook:**
  [`../skills/a2a-bridge-operator/SKILL.md`](../skills/a2a-bridge-operator/SKILL.md)

This file is the durable program cursor. A new session should be able to start here, find the exact
active slice, open its implementation plan, and continue without reconstructing the July 2026 incident
history. The detailed design remains normative for R2; this roadmap owns sequencing, status, handoff,
and completion evidence. It is the sole volatile release-status cursor. The active design and plan mirror
review-boundary evidence; `AGENTS.md`, CLI help, onboarding, and the operator skill are stable behavior/runbook
surfaces that point here and do not duplicate changing commit hashes or gate totals.

## Dependency graph

```text
R2a provenance (MERGED)
  -> R2b0 contract clarifications (MERGED)
  -> R2b1 diagnostic types + rollback-safe persistence surface (MERGED)
  -> R2b2 ACP/Fable lifecycle evidence + no-replay/warm-session safety (MERGED)
  -> R2b3 API/provider mapping + remaining container/dispatch observation (MERGED)
  -> R2c explicit one-turn billable smoke (MERGED)
       -> R2d local non-billable fallback plan (APPROVED / PENDING MERGE)
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
| R2b2 — ACP/Fable lifecycle diagnostics | **MERGED** at `0627e911` (2a `4ed12f1`; 2b `f40096df`; 2c `40790720`; 2d `14402f8`; final folds `a459b31`/`e63d4d0`; closure re-review 2 `APPROVE` at `0c0e3fe`; exact **1,100 / 0 / 0**; full host workspace **1,816 / 0 / 12 ignored**; hygiene **37/7**) | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Observer/registry, ACP evidence, owner threading, concurrency-qualified warm cleanup, then aggregate cold-path closure; one final merge boundary. |
| R2b3 — API/container diagnostics | **MERGED** at `afcc856c` (affected packages **602 / 0 / 1 ignored**; full host workspace **1,896 / 0 / 12 ignored**; hygiene **37/7**; initial review and closure re-reviews 1–3 `REVISE`; four review folds; closure re-review 4 `APPROVE` at `492946c`; final status re-review `APPROVE` at `afcc856c`) | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Independently reviewed implementation after R2b2. |
| R2c — live smoke | **MERGED** at `be54bc51` by PR #28 (initial Fable/xhigh review `REVISE`; closure re-review `APPROVE` at `0e3b8ce`; attempt 1 rejected for initial `0644`; permission-fold review `APPROVE` at `23384622`; create-new closure review `APPROVE` at `ffb7e891`; full host workspace **1,933 / 0 / 12 ignored**; separately authorized attempt 2 on `1c9e4a43` passed artifact-exact in 8.770 s with mode `0600`, exact terminal `PONG`, no retry/fallback, and clean teardown) | [R2c implementation plan](superpowers/plans/2026-07-11-r2c-live-smoke.md) | Deterministic command/artifact gates first; then one explicit, bounded, billable turn with no retry. |
| R2d — fallback plan | **APPROVED / PENDING MERGE** on `agent/reliability-r2d-fallback-plan` (initial review and closure re-reviews 1–7 `REVISE`; closure re-review 8 `APPROVE` at `1586f24`; v23 planner **24/0**, smoke **22/0**, local-file **7/0**, Linux planner **24/0** + local-file **7/0** + guarded composition **1/0**; full workspace **1,985/0/12 ignored**, hygiene **37/7**) | [R2d implementation plan](superpowers/plans/2026-07-11-r2d-local-fallback-plan.md) | Local plan only; complete smoke-v2/current-config/exact-cleanup evidence; exact trusted cwd and source-mount persistent-object identities; action-time config/executable/cwd/source/target guard; guarded host composition and child cwd use only the pinned repo object and never consult the degraded runtime. |
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

Allowed status values are `NOT STARTED`, `IN PROGRESS`, `IN REVIEW`, `APPROVED / PENDING MERGE`,
`MERGED`, `BLOCKED`, and `DEFERRED`. Update this table in the same PR that changes a slice status. Never
mark `MERGED` from a local commit or open PR.

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

- R2c merged through PR #28 at `be54bc51bf1d54df028d44f0cbd8dfdf45f779d7`. R2d is active on
  `agent/reliability-r2d-fallback-plan`, based directly on that merge head.
- R2d adds a default-off unsandboxed-ACP target capability and a local non-billable `fallback-plan`.
  The planner accepts only a complete failed smoke-v2 regular-file artifact bound by canonical path and
  exact-byte SHA-256 to the current pinned registry-only config. It rejects task envelopes and smoke-v1,
  requires an independently supplied exact canonical trusted cwd that agrees with artifact evidence and
  remains within the current source entry's canonical read-only sandbox mount, emits a schema-v2 plan,
  and never resolves, spawns, prompts, performs network/runtime probes, or executes its output.
- An eligible output is a new distinct fixed-PONG verification smoke, not an original-task retry. Its
  absolute candidate-binary argv is guarded by executable/config SHA-256, source agent/mount/mode, and
  the target's current eligibility marker plus the exact plan-time canonical cwd. The later smoke
  revalidates the closed guard before spawn and, because the target is unsandboxed ACP, performs no
  container recovery/sweep and records that backstop as `not_needed`; drift fails closed.
  Source/config/executable reads reject symlink, FIFO, device, socket, oversized, and descriptor/path
  replacement inputs. Absolute host, named, and anonymous volume forms share one grammar; `~/` is rejected
  because direct runtime argv does not expand it. External post-failure probes were removed.
- The initial bridge-mediated Sol/xhigh review of exact `b6424d725e56d1f3fde0b7c29b6057155d69dacd`
  returned `REVISE` with the nine findings recorded in the R2d plan and design v15; closure re-review 1 of
  exact `0b05c409cbbf9441348b2719a537f8f4978216a3` also returned `REVISE` with four new findings. Design v16
  closed them plus exact-cwd/runtime-dependency hardening and passed full gates at reviewed candidate
  `c8d17b2`, but closure re-review 2 returned `REVISE` with exact-cwd identity, full-diagnostic equality,
  provenance secrecy, and cleanup-evidence findings. Design v17 folds all four plus adjacent structured
  model/mode secrecy. Closure re-review 3 of exact `69152d7360a4900fe49390338b56efd94c784495`
  adjudicated all four v17 findings `FIXED`, kept adjacent complete-artifact secrecy `PARTIAL`, found no
  `SMELL`, found three new `WRONG` items, and returned `REVISE`. Design v18 binds plan/action/spawn to a
  pinned cwd directory object, sanitizes selected-entry request fields before every early return, and
  validates tagged-redacted authentication through the exact production serializer/redactor. Planner
  and smoke units pass **22/0** each, the full workspace passes **1,979/0/12 ignored** across 69
  executables, and format/diff, check, warnings-denied Clippy, release, and hygiene **37/7** are clean.
  Closure re-review 4 of exact `349755ed8f4534db0e04b8af006ca6072e01110b` returned `REVISE`: only
  device/inode survived the plan/action gap, serializer-impossible cleanup could authorize a command,
  stable operator surfaces did not explicitly name the status authority, and one source comment described
  the old relative cwd. V19 introduced a descriptor-derived persistent-object fingerprint in the closed
  guard and fails closed where unavailable, requires exact ordinary pre-spawn cleanup evidence, names this
  roadmap as the sole volatile status cursor, and corrects the comment. V20 additionally binds Darwin's
  persistent file ID to its volume UUID and Linux's opaque handle to a valid boot ID plus
  `AT_HANDLE_MNT_ID_UNIQUE`; older kernels/filesystems fail closed instead of using a reusable mount ID.
  Focused v20 gates pass planner **23/0**, smoke **22/0**, and local-file **7/0**; Linux passes planner
  **23/0** and local-file **7/0**; the full workspace passes **1,983/0/12 ignored** across 69 executables;
  format/diff, all-target check, warnings-denied Clippy, release, and hygiene **37/7** are clean. Closure
  re-review 5 of exact `49716473cf405b272dd8ecff554630b90faed0e0` adjudicated all four prior findings
  `FIXED`, then returned `REVISE` for an unbound plan-time source-mount object, the overview's stale v18
  queue, and the missing `AGENTS.md` authority link. V21 carries the mount's canonical path plus durable
  identity through the 12-field guard, refuses symlink retargeting and fingerprint drift before spawn,
  replaces the copied queue with a roadmap pointer, and aligns `AGENTS.md`. Focused v21 gates pass planner
  **24/0**, smoke **22/0**, and local-file **7/0** on macOS and planner **24/0** plus local-file **7/0** on
  Linux; the full workspace passes **1,984/0/12 ignored** across 69 executables. Closure re-review 6 of
  exact `379c3acc199fb58e6d6e1a8a8318470737ce6e8c` adjudicated all three v21 findings `FIXED`, then
  returned `REVISE`: a marked target's static cwd alias could still be dereferenced during native MCP/Kiro
  composition after source authorization, and the top next action named an already completed commit step.
  V22 selects and preserves the pinned object-addressed cwd before every guarded composition input,
  ignores target static-cwd aliases, retains ordinary canonicalization, and aligns the next action. Its
  production-spawn regression failed pre-v22 with the broadened path in the real adapter argv and now
  passes with object-addressed cwd composition on macOS and Linux. Focused
  planner/smoke/local-file totals remain **24/0**, **22/0**, and **7/0**; the Linux guarded-composition
  regression passes **1/0**; the full workspace passes **1,985/0/12 ignored** across 69 executables.
  Closure re-review 7 of exact `7fec898b5157603ae2eccd121e8367ff1914949b` adjudicated both v22
  findings `FIXED`, found no code defect or `SMELL`, and returned `REVISE` only because the current
  roadmap/plan/design ledgers did not state that Linux regression's explicit **1/0** total. V23 aligns
  that exact total across all current review-boundary evidence.
  Adapter-only, non-prompt probes also proved the macOS object path through Codex ACP 1.1.2 and Claude
  Agent ACP 0.44.0 `initialize` + `session/new`. Closure re-review 8 of exact
  `1586f24b17f5d7a7561642900fdccc9bba5fcb53` adjudicated the sole review-7 ledger finding `FIXED`,
  found no new `WRONG` or `SMELL`, and returned `APPROVE`; R2d is `APPROVED / PENDING MERGE`. No Fable,
  Claude model/Haiku, or live smoke ran; the recorded Sol/xhigh reviews are the only provider turns in
  this closure chain.

- R2b3 is implemented at `ed172ee726c06c3ee2e3f363c80178d367f8834a` with four review folds on
  `agent/reliability-r2b3-api-container`, based on `origin/main` at
  `2e9ed6408162c5af760c70c9d27237330429e81a`. The branch adds the API prompt acceptance
  barrier, bounded provider-error parsing and exact HTTP/ACP mapping, shared joinable container reaping,
  cold/cache-miss/reuse observation, and observed cleanup across ACP `:ro` and `container_rw` paths.
  Focused regressions include pre-change-red first-send ordering, provider conflict/unknown boundaries,
  structured retry/reset rejection, cold spawn-failure cleanup joining, retirement-before-cancel, typed
  reap failures, concurrent joiners, and detached cleanup. Fresh Sol/xhigh review 1 returned `REVISE`:
  cancel/retire could lose a cold or warm reservation; a session-only cleanup map could overwrite an old
  generation; warm backend drop leaked; `ContainerReap` broke its public literal shape; and real panic /
  production-timeout tests were absent. Its claim that observer failure should lose to a settled cleanup
  failure is not a defect: the design explicitly keeps a real journal persistence failure authoritative;
  container and ACP regressions now lock that precedence while retaining the typed controller result.
- The first review fold makes reservations own generation-bound controllers, seals retirement under the same
  admission lock, rejects stale promotion/dispatch, fences a later generation until checked cleanup
  acknowledges the prior owner, joins cold Forget, starts warm cleanup from `Drop`, restores the exact
  public `ContainerReap { runtime, name, reap_fn }` shape while injecting a
  private typed production controller, and proves synchronous/asynchronous panic capture plus production
  timeout child killing. Its earlier ACP checked-release process-ownership claim is superseded by design
  v14 and the fourth fold. Self-audit added cancel-during-turn-configuration coverage for cold and warm paths.
  Fresh Sol/xhigh closure re-review 1 on `51dad0130998ffb5e3598e67a0df7ca1efba9a39` confirmed those
  findings closed but retained the final check-to-inner-prompt race: cancel/retire could return while the
  winning inner prompt was still installing. It also found that clean SSE EOF without terminal evidence
  was accepted and that the design header's gate totals were stale. Self-audit separately proved ACP and
  container cleanup could still be suppressed if a release waiter was canceled before an async lifecycle
  or state snapshot completed.
- The second fold gives each exact container generation one dispatch gate. Prompt holds it only through
  inner stream installation; container teardown starts generation-owned reaping first, joins the gate,
  removes only the matching generation, and cancels the installed inner before returning. API SSE now
  rejects clean EOF without `[DONE]` or `finish_reason` while retaining finish-reason-only compatibility.
  Ten deterministic pre-change-red regressions cover all four cold/warm cancel/retire schedules, the
  then-current ACP cleanup-start contract, both container cleanup-start schedules, incomplete EOF, and its
  terminal negative control.
- On exact second-fold head `99cf8b02c73edc42b93dae6792a8701a5df13192`, the affected gate passed
  **594 / 0 / 1 ignored** (ACP 210, API 58 plus one ignored local Ollama test, container 48, core 278), and
  the host serial workspace passed
  **1,888 / 0 / 12 ignored** across 66 test/doc-test executables, workspace/all-target check, all-target
  warnings-denied Clippy, release build, and repository hygiene (**37** tracked artifacts / **7** validated
  example configs). Closure re-review 2 accepted that supplied evidence; no R2c smoke ran.
- Fresh Sol/xhigh closure re-review 2 on `99cf8b02c73edc42b93dae6792a8701a5df13192` marked all five
  inherited findings `FIXED`, then found one `WRONG/BLOCKER`: cancel/release/retire could start and settle
  the one-shot `rm -f` while the asynchronous spawn was still parked before resource creation. When spawn
  resumed, it could create the named container after removal; exact-generation rejection called the same
  already-settled controller and the writable container survived.
- The third fold gives each generation a cancellation-safe spawn-settlement fence. Teardown synchronously
  launches observer-free cleanup before its first await, but the one removal attempt waits until the spawn
  future returns or is dropped, so removal cannot be followed by late creation. Cold-cancel and warm-retire
  regressions failed **0/2** before the fold and now prove exactly one post-spawn removal; an abort edge
  proves dropping a parked spawn opens the fence. The fold also keeps off-runtime Drop alive through the
  bounded worker instead of losing a nested detached task.
- Fresh Sol/xhigh closure re-review 3 on `a3cafe61e810bcf93bad23095d162c9ff0b3e1ad` marked the inherited
  spawn-settlement blocker `FIXED`, then returned `REVISE` with one `WRONG/BLOCKER`, one `WRONG/MINOR`,
  and one `SMELL/MINOR`: per-session ACP release reaped the process shared by S1/S2; same-object duplicate
  ACP JSON members collapsed before classification; and checked release lacked direct cold/warm
  prompt-installation races.
- The fourth fold makes ACP release session-scoped. Spawn failure, escalation, registry retirement, and
  backend `Drop` exclusively own ACP process/container cleanup. Two shared-backend regressions failed
  **0/2** before the fold and prove S2 remains promptable both after S1 release and while S1 is released
  during accepted S2 work. Production ACP `spawn`/`from_child` validates unique raw JSON members before SDK
  decoding; the duplicate regression failed **0/1** before the fold and includes a distinct-path negative
  control. Cold/warm checked-release dispatch tests mutation-fail **0/2** with only the gate waits removed
  and pass **2/2** after restoration.
- The fourth-fold affected gate passes **602 / 0 / 1 ignored** (ACP 213, API 58 plus one ignored local
  Ollama test, container 53, core 278), and host serial workspace passes **1,896 / 0 / 12 ignored** across
  66 test/doc-test executables. Workspace/all-target check, warnings-denied all-target Clippy, release
  build, format/diff, and repository hygiene (**37** tracked artifacts / **7** validated example configs)
  are clean.
- Fresh bridge-mediated Sol/xhigh closure re-review 4 on
  `492946cbb28ec624aa6b43a9a059581ef5f84538` adjudicated all three inherited findings `FIXED`, found no
  new `WRONG` or `SMELL`, and returned `APPROVE`. It accepted the supplied exact-tree gates rather than
  rerunning them and confirmed no R2c/R2d/parked-issue scope entered the branch.
- The approval-recording fold reran the full workspace **1,896 / 0 / 12 ignored** and hygiene **37/7**.
  Its first status-only Sol/xhigh review returned `REVISE` on `15a5ed97` for a contradictory top cursor.
  The amended `afcc856c` fold made the roadmap top, table, vocabulary, plan, and design agree on
  `APPROVED / PENDING MERGE`; it reran the same full/hygiene gates, and targeted Sol/xhigh re-review
  adjudicated the mismatch `FIXED`, found no new findings, and returned `APPROVE`. The branch was then
  fast-forwarded and pushed to `origin/main` at `afcc856c3276fe682fb78dc657591021f5e604fc`. At that R2b3
  merge checkpoint, R2c was unrun and required separate operator authorization.
- `origin/main` contains R2b3 at `afcc856c3276fe682fb78dc657591021f5e604fc`. R2b2d was approved at
  `14402f895a5eda2852684a8fbd35f83452e2645f`; the final full-branch review fold is committed at
  `a459b31de5a4665138a7330868e38dfb8992438b`, and the re-review-1 fold at
  `e63d4d085e8dd51424cdedebda7aa64b9f1a8b01`. Fresh Sol/xhigh closure re-review 2 returned `APPROVE`
  on published head `0c0e3feefa8d66169d4ee18faa9911d5fb1a32d8`; a final docs-only Sol/xhigh re-review returned
  `APPROVE` on `0627e911`; R2b3 subsequently merged at `afcc856c`.
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
- R2b2 was fast-forwarded to `origin/main` at `0627e91144e79d9328ed9b5635033cf410c9e96e`; R2b2a is at
  `4ed12f1035c16fa5dbd55169e59ca4c277373da4` and R2b2b at
  `f40096dfcfb43a37236ce5626fd362a16645f0fe`. R2b2c owner/workflow authority is committed and pushed at
  `407907202982d732c2395be0f6319f6029622f82` after final review 7 `APPROVE` and exact-tree full gates;
  R2b2d is approved/pushed at `14402f895a5eda2852684a8fbd35f83452e2645f`, and the aggregate final-review
  folds are committed at `a459b31de5a4665138a7330868e38dfb8992438b` and
  `e63d4d085e8dd51424cdedebda7aa64b9f1a8b01`. Fresh Sol/xhigh closure re-review 2 returned `APPROVE`
  on published head `0c0e3feefa8d66169d4ee18faa9911d5fb1a32d8`; the final docs-only Sol/xhigh re-review returned
  `APPROVE` on the merge head; R2b3 subsequently merged at `afcc856c`.
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
  limitation and bounded yield polling remain minor test-coverage follow-ups. At that R2b2 checkpoint, no
  docs-link checker was present and no live/billable gate had run.
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
- Full-R2b2 closure re-review 1 adjudicated ready-result precedence and explicit terminal disposition
  `FIXED`, but cold cleanup `PARTIAL`: successful/canceled/fatal paths were closed, while transient configure,
  prompt-open, and stream failures still discarded a failed result-bearing Release/Forget and admitted the
  next attempt. Production registry invalidation is asynchronous, so it could not make that overlap safe. The
  review returned `REVISE` with one `WRONG/MAJOR` and one branch-completeness `SMELL/MAJOR`.
- Fold `e63d4d085e8dd51424cdedebda7aa64b9f1a8b01` carries explicit cleanup retry eligibility with every
  `Attempt::Transient`. A resolve failure has no session and remains retryable; configure, prompt-open, and
  stream failures retry only when their observed cleanup succeeds. Cleanup failure terminates after the
  current attempt while preserving the original transient error as primary. Each of the three schedules
  reproduced red as `Completed` with a second attempt and now fails after one resolve/configure, using the
  same observer for one Release. Prompt-open cancellation, non-transient configure cleanup, and Text,
  Permission, and Usage one-item controls close the review's remaining mutation gaps. Workflow passes
  **86 / 0 / 0**.
- The second folded six-package gate passes **1,100 / 0 / 0 ignored**; the host serial full workspace passes
  **1,816 / 0 / 12 ignored**. Format/diff, workspace/all-target check, warnings-denied workspace/all-target
  Clippy, workspace release build, and hygiene **37/7** are clean. No live/billable gate ran. Run full-R2b2
  closure re-review 2 before merge.
- Fresh Sol/xhigh closure re-review 2 inspected every line in the retry-veto fold plus the relevant trait,
  registry, Worktree, retry, terminal, and test surfaces. It adjudicated the transient-cleanup
  `WRONG/MAJOR` and branch-completeness `SMELL/MAJOR` `FIXED`, confirmed the earlier ready-result and terminal
  folds remain closed, found no new findings, and returned `APPROVE`. The retained Git-fixture and bounded-yield
  debts remain minor. Exact published head was `0c0e3feefa8d66169d4ee18faa9911d5fb1a32d8`; no live/billable gate
  ran, and no docs-link checker is present. R2b2 was fast-forwarded to `origin/main` at
  `0627e91144e79d9328ed9b5635033cf410c9e96e`; begin R2b3.
