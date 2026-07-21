# R2f owner design — Sol adversarial review 1

- **Verdict:** `R2F OWNER DESIGN: REVISE`
- **Date:** 2026-07-20
- **Execution:** operator-served host Codex, read-only, single agent, about 15 minutes
- **Requested/catalog-advertised identity:** raw `gpt-5.6-sol`, `xhigh`, `read-only`
- **Operator release:** `3c02bf3f419da8bc`
- **Adapter/CLI:** `@agentclientprotocol/codex-acp` 1.1.2 / `@openai/codex` 0.144.1
- **Evidence limitation:** the deployed unary submit path created no durable task or turn-log row, so requested and
  successfully configured identity is retained but there is no separate ledger observation of effective identity.
- **Method:** fresh clean-room review through the operator-owned bridge; no helper, prior review, build, test, edit,
  provider/model call, or earlier-review conclusion was available to the reviewer

## Frozen boundary

`HEAD`, base, and merge base were all `345941db91a7d898884bfe79e573433484ccafcc`. The reviewer verified exactly
the following two modified and three untracked files and their SHA-256 values before analysis:

| Path | SHA-256 |
|---|---|
| `docs/reliability-execution-roadmap.md` | `49610bab5037ef3f10ab3a1c2eef476066db552906a514a8d70cd4a602ec5d46` |
| `docs/superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md` | `48d2df3808ff1d1dcd7abde69654f184c81ab2464f49847f28aafbc9823e1fc6` |
| `docs/superpowers/plans/2026-07-20-r2g-stable-ingress.md` | `e045470ac7af477fa16f8e8c81ae188016a6350e132a1532f2225874ebc45704` |
| `docs/superpowers/specs/2026-07-20-r2f-owner-design.md` | `e64881c509988bbf2fd3b84f82af2d11fd53b62e57b62ff0dde40df89c3121ed` |
| `docs/superpowers/spikes/2026-07-20-r2f-short-bound-validation.md` | `aadba79dba885a40b28d9026af48d5379f7cdf746afa23634b4343927dcb8dc5` |

The reviewer read `AGENTS.md`, the complete operator skill and routed references, all five review surfaces, and the
current workflow, coordinator, ACP, registry, session, SQLite, process-supervision, and worktree code needed to test
the design claims. The checked-in spike results were accepted as documentary evidence and were not rerun.

## WRONG findings

### 1. High — R2f1b can destroy useful work before the preservation slice exists

A worktree-backed cold node can have useful uncommitted edits when R2f1b's new absolute deadline cancels it. The
design enables deadline behavior in R2f1b but assigns explicit worktree preservation to later R2f2. Current
cancellation invokes `ColdCleanupAction::Forget` in `bridge-workflow/src/executor.rs`; both worktree cleanup strengths
invoke provider removal in `bridge-worktree/src/backend.rs`; production removal is `git worktree remove --force` in
`bridge-worktree/src/provider.rs`. Implementing the documented order would delete state that invariant 8 promises to
preserve.

**Required correction:** add a distinct non-destructive canceled/takeover cleanup operation before any automatic
deadline. It must settle or transfer session/process ownership without worktree removal. R2f2 may add the takeover
artifact and resume UX later.

### 2. High — protected retention makes the exactly-one terminal ledger contract impossible

When the ledger reaches 100,000 rows or 128 MiB and every terminal row is pinned, D6 forbids collection and allows
telemetry admission to fail without changing the workflow outcome. The same design requires one authoritative
summary, transactional coupling to task/offline state, and terminalization of the pre-created attempt row exactly
once. The implementation must exceed the cap, leave the attempt nonterminal, fail primary terminal state, or commit
primary state without the claimed summary; every result violates one stated rule.

**Required correction:** define a satisfiable reservation/admission mechanism and an explicit terminal observability
failure contract that cannot rewrite primary outcome. State whether capacity exhaustion rejects anything before
provider effects and how primary terminal state survives summary enrichment failure.

### 3. High — the two-hour execution budget cannot include its post-deadline tail

D1 waits until the two-hour deadline before canceling, while D11 then permits 60 seconds for cleanup and 10 seconds
for terminal persistence. Terminal observation can occur near 2:01:10 even though D3 says the execution budget
includes cleanup/finalization and D4 calls two hours the absolute execution deadline.

**Required correction:** name the node-work cutoff and end-to-end terminal bound separately. Either publish 2:01:10
after a two-hour work cutoff or reserve the 70-second tail and stop node work at 1:58:50 for a two-hour end-to-end
bound.

### 4. Medium — `fixed_grace` contradicts the frozen-sibling-clock invariant

With a sibling's frozen 60-minute deadline, another root failing at minute one and a five-minute `fixed_grace`
cancels the sibling at minute six. That is a deliberate shortening even though invariant 3 says a failed fan-out leg
does not shorten a sibling's frozen clocks.

**Required correction:** scope invariant 3 to `bounded_independent`; define `fail_fast` and `fixed_grace` as
pre-prompt frozen policy exceptions whose separate failure-triggered clocks do not mutate the recorded node deadline.

### 5. Medium — plan and roadmap do not literally represent the approved design/order

The design defines `0a/0b/1a/1b/2/3a/3b/3c/4`; the linked plan retains coarse R2f0-R2f3 headings, no explicit R2f1a
owner, stale instructions to run already-complete owner design, and one common run id instead of stable
`execution_id` plus per-resume `attempt_id`. The roadmap top says D1-D11 are approved while later next-action and
dependency text says owner decisions/bounds remain open.

**Required correction:** make the execution plan, roadmap cursor/dependency graph, identities, and exact slice order
literal and remove the stale owner-design prerequisites.

## SMELL findings

### 1. High consequence — process-reuse safety lacks an atomic signaling capability

A start-time check followed by `kill` retains a PID/PGID reuse window, and concurrent automatic cleanup/manual
takeover has no specified single-flight boundary. Existing compatibility supervision demonstrates the stronger
retained group-anchor pattern.

**Required correction:** require a spawn-time retained child/group capability, journal-before-effect, and one joined
cleanup/takeover flight. Missing capability produces typed refusal/partial disposition, never late PID/name signaling.

### 2. High consequence — D8 health states are not closed against registry states

D8 uses `suspect`, `auth_required`, `degraded_external`, `isolated_unknown`, `quarantined`, and `probation`; the
registry list names only `active`, `draining`, `quarantined`, `dead`, and `retired`. Orthogonal axes, illegal
combinations, restart/cooldown behavior, successor precedence, and exact-generation probation routing are undefined.

**Required correction:** add a complete lifecycle-by-health transition and routing table.

### 3. High consequence — close debt/capacity recovery lacks a crash-safe state machine

The design requires close single-flight, generation-owned durable debt, and 1/5/30-minute retries but does not define
the durable key/store, write-before-close order, boot reconstruction, post-third-retry state, or capacity ownership
after unknown close disposition.

**Required correction:** specify a durable `(generation_id, session_id)` debt machine with idempotency key, attempt
ordinal, capacity disposition, retry/exhausted state, boot reconciliation, and generation-retirement rules.

### 4. Medium — compatibility profile and phase thresholds are unnamed

Omitted workflow policy promises a bounded compatibility profile, but only the high/xhigh review warning/work cutoff
is numeric. No finite phase thresholds or non-review/default behavior is named, forcing implementation to invent
policy or reject an omission that the design says is supported.

**Required correction:** provide the checked-in profile vocabulary, exact defaults, phase-warning values, task-class
mapping, and unknown-class validation behavior.

## Unverified implementation assumptions

- The 31-second evidence covers the local ACP spawn/initialize hang, not arbitrary provider/network latency or a
  future provider-specific health RPC.
- The six-second evidence does not yet exercise the exact stacked ACP grace plus process/container/collateral path.
- The 60-second envelope has no current typed partial/durable-owner transfer, and the 10-second evidence covers the
  current terminal store rather than the proposed workflow-summary sink.
- Half-open and debt-retry scheduling remain policy prototypes; production restart/exactly-once behavior is absent.
- Adapters must prove truthful `session/close`, acknowledgement/capacity semantics, and mechanically proved dead.
- Warm drain must prove restart/reload, concurrent checkout, TTL expiry, close debt, and no forced retirement.
- R2g's stable ingress deferral is appropriate and was not reported as an R2f defect.

R2F OWNER DESIGN: REVISE
