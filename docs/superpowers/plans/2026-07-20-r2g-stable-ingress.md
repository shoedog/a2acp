# R2g — Stable ingress and side-by-side bridge release handoff plan

- **Status:** QUEUED AFTER R2f; scope boundary approved, focused owner design not started
- **Prerequisite:** R2f3c handoff contract and R2f lifecycle/affinity primitives
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **R2f owner design:** [`../specs/2026-07-20-r2f-owner-design.md`](../specs/2026-07-20-r2f-owner-design.md)

## Why this is a separate increment

R2f can rotate backend adapter instances inside one bridge process. It cannot make replacement of the bridge binary
itself non-disruptive. Current `serve` has three single-process ownership assumptions:

1. one process binds the configured TCP endpoint;
2. one process holds an exclusive advisory lock on the configured SQLite task store;
3. warm sessions, running workflow producers, broadcast tails, and live SSE streams have process-local owners.

Launching a second binary on the same port/store therefore fails or creates ambiguous ownership. Restarting the old
binary terminates those in-memory owners. Socket activation alone does not preserve task/session affinity or move an
existing SSE producer. R2g needs a focused operator design rather than an incidental process-launch patch.

## Outcome

Provide one stable local operator endpoint while two exact bridge releases may coexist:

- existing running tasks, SSE subscriptions, and warm session/context ids remain affine to the predecessor;
- new work routes to the promoted successor;
- neither release dual-opens today's exclusively owned task store;
- rollback and drain are explicit, observable, and do not replay provider work;
- a predecessor exits only after its exact process-local ownership is settled or the local OS owner takes an
  explicit exceptional action.

Until R2g is implemented and gated, production binary replacement still requires a coordinated pause. R2f backend
generation rotation must not be presented as closing this process-level gap.

## Invariants inherited from R2f

- Never stop a running turn or warm session merely to make deployment appear complete.
- Never infer affinity from process names, ports, repository paths, model ids, or timestamps.
- Never replay a prompt, retry a task, migrate ACP context, or silently create a replacement session.
- New release readiness does not imply promotion; promotion does not imply predecessor retirement.
- Missing or conflicting affinity fails closed with an operator-visible recovery locator.
- The stable endpoint preserves authentication and does not broaden remote destructive authority.
- Every release, process, store, task, execution, session, and stream identity is bounded and auditable.
- Cross-version schema compatibility is proved before either release accepts traffic.
- Rollback cannot route new work to a release whose store or protocol state is incompatible.

## R2f3c prerequisite contract

R2f3c should expose, without implementing ingress:

- stable bridge release and process-instance ids;
- readiness state distinct from liveness;
- accepting-new-work versus draining state;
- task, execution, attempt, context/session, and backend-generation ownership locators;
- a bounded list/count of running turns, warm contexts, live task producers, and unresolved cleanup/debt;
- exact refusal reasons when a request cannot be served safely by that process;
- terminal drain observation that cannot become true while process-local ownership remains;
- versioned local status suitable for a future ingress without exposing prompts, output, credentials, or arbitrary
  command lines.

## Focused owner-design questions

Settle these before R2g source implementation:

1. **Ingress owner:** stable launchd-owned local router, inherited listener supervisor, or another operator-owned
   process; define restart and authority behavior for the ingress itself.
2. **Affinity ledger:** which stable ids route each RPC/SSE reconnect, where mappings are persisted, and what happens
   after ingress crash or mapping ambiguity.
3. **Store topology:** per-release/per-process stores with routed ownership, a single storage broker, or a redesigned
   multi-process store. Sharing the current SQLite path is forbidden by its exclusive lock.
4. **Protocol/version negotiation:** ingress-to-release API, readiness, schema compatibility, and mixed-version
   support window.
5. **Streaming:** preservation of established SSE connections and deterministic reconnect using task id plus cursor.
6. **Promotion/rollback:** candidate admission, atomic new-work cutover, rollback eligibility, and failure semantics.
7. **Drain/GC:** warm-owner visibility, predecessor retention, urgent-security behavior, release artifact retention,
   and exact last-owner proof.
8. **Operator UX:** foreground dogfood, launchd production ownership, status, notifications, and pause fallback.

## Provisional implementation slices

These are routing aids, not an approved technical design:

1. **R2g0 — focused owner design and threat model:** freeze ingress/store/affinity architecture and failure matrix.
2. **R2g1 — provider-free ingress and affinity ledger:** stable endpoint, exact release registration, readiness, and
   deterministic request routing with no provider execution.
3. **R2g2 — task/SSE/store continuity:** existing-task and cursor affinity, crash recovery, schema gates, and strict
   ambiguous-owner refusal.
4. **R2g3 — promotion, rollback, and drain:** side-by-side release lifecycle, new-work cutover, warm predecessor
   retention, terminal drain proof, and bounded release cleanup.
5. **R2g4 — operator integration and closure:** launchd/foreground runbook, production-safe upgrade/rollback drill,
   deterministic full matrix, and separately authorized live dogfood.

## Deterministic design and verification matrix

The focused design must cover at least:

- predecessor running while successor becomes ready;
- successor readiness failure before promotion;
- new-work cutover concurrent with task/session creation;
- existing warm continuation and new cold work during drain;
- SSE connected across promotion and reconnecting by cursor afterward;
- ingress crash with recoverable, missing, stale, and conflicting affinity records;
- predecessor crash before and after terminal persistence;
- successor crash followed by eligible and ineligible rollback;
- incompatible store/schema/protocol versions;
- old and new releases attempting the same SQLite store;
- indefinitely active warm predecessor and explicit urgent-security freeze;
- release cleanup racing late reconnect or retained task/session ownership;
- unrelated local services and bridge releases surviving scoped cleanup.

No provider turn is needed to prove routing, affinity, store exclusion, crash recovery, promotion, rollback, or drain.
Any live gate is separately selected and authorized after deterministic tests and adversarial review are green.

## Completion boundary

R2g is complete only when the stable endpoint can promote and roll back side-by-side releases while deterministic
tests prove running/warm/task/SSE affinity, store safety, crash recovery, and last-owner drain. Documentation must
state exactly what was exercised in production. A successful backend-generation rotation, quiet-period restart, or
one clean upgrade is not sufficient evidence.
