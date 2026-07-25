# R2f owner design — Sol closure review 1

- **Verdict:** `R2F OWNER DESIGN: REVISE`
- **Date:** 2026-07-20
- **Execution:** operator-served host Codex, read-only, single clean-room turn, about 12 minutes
- **Requested/catalog-advertised identity:** raw `gpt-5.6-sol`, `xhigh`, `read-only`
- **Operator release:** `3c02bf3f419da8bc`
- **Adapter/CLI:** `@agentclientprotocol/codex-acp` 1.1.2 / `@openai/codex` 0.144.1
- **Method:** no helper, edit, build, test, nested agent, nested provider/model turn, or production mutation by the
  reviewer; current source was inspected as needed and the checked-in spike was accepted only as documentary evidence

## Frozen boundary

`HEAD`, base, and merge base were all `345941db91a7d898884bfe79e573433484ccafcc`. The full changed boundary was
two modified and four untracked documentation files, with no other changed path:

| Path | SHA-256 |
|---|---|
| `docs/reliability-execution-roadmap.md` | `e6294c1f56bc22849a9fafe0625400a4195236f6078c9b02d3888c57b56c2c9d` |
| `docs/superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md` | `a570016cbc85ab30cff940166c3ae31fca4398414b8272ff37a883d11906eb92` |
| `docs/superpowers/plans/2026-07-20-r2g-stable-ingress.md` | `e045470ac7af477fa16f8e8c81ae188016a6350e132a1532f2225874ebc45704` |
| `docs/superpowers/reviews/2026-07-20-r2f-owner-design-sol-review-1.md` | `2623e40ea9f90c63262e596d73de19c745233f45760e3355a58b451c6dd463c6` |
| `docs/superpowers/specs/2026-07-20-r2f-owner-design.md` | `7d416854559f9499bd850a086b7618771229648c7150acfaabd9757756acf080` |
| `docs/superpowers/spikes/2026-07-20-r2f-short-bound-validation.md` | `2ce34a150d45e303f4162d425fe1aecf324cccf5f21c314c595d7a6c6b89b484` |

The reviewer read `AGENTS.md`, the complete operator skill and routed references, all six frozen files, and relevant
workflow, ACP, registry, process, worktree, session-manager, and SQLite code. The immutable boundary was rechecked
after semantic inspection.

## Inherited-finding adjudication

1. `FIXED` — deadline/worktree ordering. `preserve_after_cancel` and recovery ownership are now prerequisites before
   timers and live in R2f1b before the R2f2 UX.
2. `FIXED` — protected telemetry capacity. Bounded reservation, explicit no-row unavailable output, and independently
   ordered primary terminality form a satisfiable contract.
3. `FIXED` — two-hour arithmetic. The design separates the two-hour work cutoff from the `2:01:10` terminal bound.
4. `FIXED` — fixed-grace clock contradiction. Clock preservation is scoped to `bounded_independent`, with the other
   frozen policies recorded as explicit earlier-cancel conditions.
5. `PARTIAL` — literal plan/roadmap agreement. The primary surfaces use the right identities, status, bounds, and
   exact slice order, but one roadmap paragraph still said corrections were being folded and its current handoff
   still named `0d628271`/`agent/r2f-incident-intake` rather than the frozen current main/branch.
6. `FIXED` — PID reuse and competing automatic/manual authority. The design requires a retained spawn-time
   capability, journal-before-signal, typed refusal, and joined node flight.
7. `FIXED` — incomplete health states. D8 now provides orthogonal lifecycle/health axes, precedence, durable
   transitions, illegal combinations, successor choice, probation targeting, restart behavior, and proved death.
8. `PARTIAL` — close debt/capacity recovery. The state machine is now durable and crash-aware, but its recovered
   `close_prepared` transition incorrectly consumed retry ordinal 1 before any ordinal-0 dispatch.
9. `FIXED` — unnamed compatibility policy. D4.1 names deterministic finite profiles, selection, validation, and Max
   qualification.

## WRONG findings

### 1. High — crash-before-dispatch consumes the initial close attempt

Concrete state: `close_prepared` is durable, the bridge crashes before crossing the accepted-work barrier, and boot
reconstructs the debt. The state says no dispatch occurred but recovery schedules retry ordinal 1. If ordinals 1-3
then fail, the bridge reports initial-plus-three-retry exhaustion after only three actual dispatches.

**Required correction:** recovered `close_prepared` dispatches still-unspent ordinal 0, immediately or at its named
safe-recovery bound. Ordinal 1 begins only after a definitely failed ordinal-0 dispatch.

## SMELL findings

### 1. High consequence — shared-process action lacks a generation-scoped single-flight owner

Current ACP multiplexes sessions on one `Supervised` process, and escalation can terminate the whole connection. The
design specified only per-node process cells, so two nodes on one generation could compete to signal one retained
capability and reconcile different results.

**Required correction:** process/container fencing, signaling, and settlement use one generation-scoped flight
referenced by per-node session/worktree flights, with the one result published to every collateral node.

### 2. High consequence — durable recovery worktree ownership is not closed against sweeps

Current run-end cleanup selects sidecars by `run_id`, while boot cleanup may select same-host sidecars with a free
flock. The process-lifetime lease unlocks on clean exit or crash. Naming a durable recovery lease did not define its
persisted state, atomic rebinding, sweep exclusion, corrupt/stale behavior, or exact final release.

**Required correction:** define the identity-bound durable claim and atomic transition, make both sweep predicates
recognize it, fail safe on corruption/ambiguity, and name resume/final-disposition release operations.

### 3. Medium consequence — telemetry selection omits served/no-store and initial open failures

The design assigned configured stores first and the platform ledger only to offline execution, while `serve` may
validly use an in-memory task store. It also named only protected-capacity reservation failure. Served/no-store and
permission, lock, migration, corruption, or initial-open failures would force implementation to invent a ledger or
failure contract.

**Required correction:** select exactly one ledger for every surface and define bounded no-fallback unavailable
outcomes for every pre-reservation failure without changing primary execution semantics.

### 4. Medium consequence — capacity claims have no normative capacity source

The debt machine retains capacity claims and tests refusal, but neither design nor plan said whether a limit comes
from adapter capability, checked-in configuration, or an incident count. The incident expressly says fifteen is not
a threshold.

**Required correction:** define capability/config precedence, known-full admission, claim release, and truthful
behavior when no finite limit is available.

## Unverified assumptions and gate boundary

- The 31-second evidence covers a local ACP initialize hang, not arbitrary network/provider behavior or the future
  health controller.
- The six-second check covers disposable process groups, not the unimplemented dispatch fence, collateral projection,
  or durable ownership transfer.
- The 60-second typed partial-transfer path and 10-second workflow-summary sink remain unimplemented.
- Adapter close acknowledgement, idempotency, and capacity semantics require deterministic proof.
- Retained-process identity, descendant enumeration, PID/PGID reuse resistance, generation-wide settlement, and
  preserved-worktree clean-exit/crash/reboot/resume behavior require direct tests on supported platforms.
- The health controller, planned drain, R2f3c handoff, and R2g process boundary remain design-only.
- No implementation, full-suite, live compatibility, deployment, or production-operation gate was exercised.

R2F OWNER DESIGN: REVISE
