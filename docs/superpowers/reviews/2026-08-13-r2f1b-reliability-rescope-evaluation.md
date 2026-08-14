# R2f1b and reliability-program rescope evaluation

Date: 2026-08-13

Status: **EVALUATED ALTERNATIVE - OWNER DECLINED SPLIT; FULL TEN-TASK PATH
SELECTED**

Frozen green production base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`

Preserved rejected 3c2 feature head: `771c0fb8`

Superseded small A1 repair: `6616753bf479d8775381eb9ef1d7237f5660514c`

Closure-approved A1 continuation: `5cbeea1ed882afe448d3825984af9a3ed74bcb58`
(retained, not integrated)

## Historical recommendation and owner disposition

**Status update (2026-08-14):** one separately authorized bounded continuation
removed the two open A1 race mechanisms and received a clean closure approval.
After reviewing this evaluation, the owner declined its proposed split and
selected the full ten-task 3c2 path. A1 remains retained and not integrated; A2
is now the next sequential task from exact input `5cbeea1e`, but no A2
implementation run started in the decision/bookkeeping turn.

This evaluation recommended not continuing A1-A4 plus B-G as the active critical
path. Its alternative was to close the current reliability core at green 3c1
plus one small current-production cleanup/retry shield, defer the larger
automatic-reliability work, and advance explicit default-off provider work.
That recommendation is preserved as evaluated history but is not the selected
program path.

Had it been selected, this would have reduced the active 3c2 reliability repair
from ten planned implementation tasks plus aggregate review to **one bounded
shield task**. Its later automatic API deadline/restart option would have begun
with a fresh focused design and at most three implementation cuts, not the
existing filesystem journal plan.

## Options evaluated

1. **Finish the full ten-task 3c2 plan.** Continue from approved A1 through A2,
   A3, A4, and B-G, then run the combined review, fold, and CI. This builds the
   strongest automatic request tracking and crash-recovery system, but it is the
   largest and slowest option. Even after completion, production activation is
   deferred to 3d.
2. **Land A1 or A1+A2 and pause.** Reject this as an endpoint. Those tasks only
   provide low-level file identity, safe naming, directory binding, and a lock.
   No current request, cleanup, or retry behavior uses them. Landing them alone
   creates dormant code with ongoing maintenance cost and no operator benefit.
3. **Close the current reliability need narrowly.** Retain A1 outside main,
   skip A2, and implement only Shield S1 on current green main so an uncertain
   cleanup can never authorize another provider request. This was the evaluation's
   recommended near-term option; the owner did not select it.
4. **Build automatic request recovery later, if it becomes a real need.** Start
   a new focused design using the existing transactional database, the
   single-operator/quiet-update assumptions, and no automatic replay of an
   uncertain request. Cap that later program at three individually useful
   tasks. This is a replacement for the ten-task filesystem plan, not a promise
   to resume at A2.

Provider expansion was also evaluated as independent work after Shield S1 in
the proposed split: explicit, default-off OpenRouter work and read-only OpenCode
discovery do not need A1-A4 or B-F. Because the split was not selected, actual
provider implementation sequencing returns to the canonical roadmap; read-only
discovery remains separable.

## Product and threat-model basis

The application has one operator on one machine. Normal updates can stop usage,
back up state, replace the binary, run a one-way migration, and restart one
instance. The required safety model still includes ordinary crashes,
cancellation, ambiguous provider acceptance, retry, corrupt local state, and
unrelated processes. It does not currently require:

- simultaneous old/new bridge releases;
- mixed-version readers and writers;
- hot migration while requests are active;
- arbitrary same-UID peers mutating the bridge's owner-private journal
  namespace during a rename window; or
- backward compatibility with the rejected, never-armed 3c2 request-journal
  format.

This does not authorize weakening compatibility for already-landed production
history/custody schemas. It only removes compatibility obligations for a 3c2
format that has never been a production writer.

## What V3 means here

`V3` is not one database-engine version. The repository uses V3 for several
versioned contracts: structured workflow-history records, protected process and
container flights, and the proposed API request-flight route. The relevant 3c2
phrase, `resource_flight_route_v3 = None`, denotes an unarmed API
request-flight/custody protocol generation. Green main at `42249b3d` does not
even contain that route field; it serves `ApiBackend` as `LegacyV2`.

SQLite does already persist other landed V3 records. Those are separate from
the rejected 3c2 descriptor-root request journal and retain their existing
compatibility contract.

## Size and causality

The preserved `42249b3d..771c0fb8` feature delta is 3,117 insertions and 126
deletions across twelve paths; `bridge-api/src/backend.rs` alone adds 1,759
lines. The redesign ceilings are 3,500 production lines and 6,650 total changed
lines before possible B2/C/E/F2/G splits. This is a mini-program, not a bounded
1,500-line slice.

The production route remains unarmed throughout the plan. Therefore most of
A1-F is infrastructure for a future activation, not repair of current served
behavior. Tests, candidate types, and a dormant injection seam are not delivery
proof.

One G mechanism is independently live on green main: `cleanup_cold_session`
records `BackendCleanupDispositionV1`, then maps every `Ok(disposition)` to
`Ok(())`; three retry sites use `.is_ok()`. A backend returning `Ok(Unknown)`
therefore permits invalidation and another attempt. That current-production
projection collapse is separable from every A1-F journal decision.

## A1-A4 and B-G disposition

| Old task | Disposition | Reason / replacement |
|---|---|---|
| A1 | Closure-approved at `5cbeea1e`; retain, do not integrate pending owner program choice | The bounded continuation removed restoration and post-`Io` no-effect proof, closing both peer-race WRONGs without adding a namespace transaction engine. The candidate remains inactive and unarmed. |
| A2 | Remove from active plan | Trusted anchor plus sibling namespace lease exists to defend the custom file journal. A single live bridge instance and owner-private state can instead be enforced at application/store admission. |
| A3 | Remove from active plan | Rename-stage rollback/roll-forward is a custom filesystem transaction engine. If automatic request recovery is later required, use one transactional store and a fail-closed recovery policy. |
| A4 | Remove from active plan | Owned journal wrappers and deletion of candidate-only broken APIs disappear when the candidate journal is not adopted. |
| B | Defer and redesign | Bounded capacity and ordinal allocation matter only when durable automatic request admission is armed. Use a configurable small single-machine bound in a transaction; 4,096 is not a product requirement. |
| C | Defer; default to no replay | On restart, interrupted API requests become `Unknown` and require explicit operator action. Do not build an outbox/replay engine until a real workflow requires automatic recovery. |
| D | Reduce to later API-local ownership | Per-request identity and stale-cancellation fences remain useful, but can be an in-memory API concern while automatic cross-crash retry is unsupported. |
| E | Audit before provider landing | Add only a bounded Legacy cleanup cell if a current-main red proves active request cleanup can falsely claim `Complete`. Do not preinstall the rejected V3 route. |
| F | Delete | This migrates away from an adapter that exists only on the rejected feature branch. Green main has nothing to remove. |
| G | Extract now as shield S1 | Preserve the exact typed cleanup disposition and authorize retry only on exact `Complete`. This is a current-main defect independent of request journaling. |

## Shield S1 - the only active reliability implementation task

Frozen input should be the exact green integration base selected at dispatch.
Own only `crates/bridge-workflow/src/executor.rs` and directly affected workflow
tests/doubles.

Compile-correct contract:

1. `cleanup_cold_session` and `preserve_then_cleanup_cold_session` return the
   exact `BackendCleanupDispositionV1` on success while still recording the
   tracker result.
2. Every retry site authorizes invalidation/redispatch only for
   `Ok(Complete)`. `Retained`, `Preserved`, `Unknown`, and `Err` terminate the
   retry with the existing protective failure path and no later provider
   request.
3. Non-retry callers may deliberately discard the returned disposition only
   after the tracker owns it; no terminal projection is flattened.
4. A red test returns `Unknown` from cleanup after a transient failure and
   proves one attempt, zero invalidation, and zero redispatch. An exact
   `Complete` control proves the configured retry still occurs.

Stop at 160 changed production lines or 350 total. One implementation review;
one targeted repair plus one closure only for a closed enumerable rejection.
Run the full repository gate. This task does not touch A1, API request
journaling, production V3, providers, or deployment.

## Deferred automatic-reliability program

If automatic API deadline/restart behavior later becomes important, open a
separate `R-auto-request` program with these binding choices:

- one live application/store owner;
- owner-private state and quiet-period one-way migration with backup;
- no reader/writer compatibility with rejected 3c2 formats;
- transactional reservation/settlement in the existing SQLite composition (a
  trait may live in `bridge-core`, its SQLite implementation in `bridge-store`,
  and injection in the binary, avoiding a crate cycle);
- provider acceptance ambiguity is durable `Unknown`, never automatic replay;
- no arbitrary-peer namespace transaction engine; and
- maximum three individually green implementation tasks after one focused
  design: store/admission, API ownership/cleanup, and integration/projection.

3d preparation-flight activation belongs with this deferred automation program,
not on the provider-integration critical path.

## Broader program split

1. **R-core:** treat green 3s + 3a + 3b1 + 3b2 + 3c1, followed by S1, as the
   current reliability closure boundary. Keep automatic R2f1b deadlines
   disabled.
2. **R-automation/takeover:** defer 3c2, 3d, R2f2, R2f3a, R2f3b, R2f3c, and
   R2f4 from the critical path. Retain their designs as optional capability
   work, not prerequisites for explicit providers.
3. **R-zero-downtime:** defer R2g. It solves simultaneous releases, store/SSE
   affinity, promotion, rollback, and drain. Quiet-period upgrades make that a
   future convenience/availability project rather than present correctness.
4. **Provider expansion:** permit R3e OpenRouter after S1 using the existing
   explicit OpenAI-compatible API boundary, local fakes, environment-only
   secrets, no automatic fallback, and no production activation. Permit
   read-only OpenCode protocol discovery in parallel; implement R3f only after
   its exact local boundary is observed. R3d4/R3d5 automatic scheduling and
   staged activation may proceed separately rather than block explicit
   default-off provider support.

No provider live turn, running-service edit, deployment, or automatic
scheduling is authorized by this evaluation.

## Historical same-machine parallel option

The following parallel shape was evaluated for the proposed split and is not
the active sequencing decision after the owner selected the full ten-task path.

No new bridge feature is required to develop two items on one machine. ADR-0040
already permits parallel implementor flights from one frozen base with one
shared config, unique clones/containers/run leases, no sibling auto-merge, and
sequential `merge --integrate-current` plus one aggregate full suite/review.

Use at most two simultaneous write-capable flights on this machine and
serialize their full Cargo/Docker gates to avoid resource contention. Freeze
one base, assign disjoint paths, and reserve shared manifests, roadmap edits,
generated files, and integration cleanup to one integration task. S1
(`bridge-workflow`) and OpenRouter discovery/config work
(`bridge-api`/composition/compatibility) are viable siblings if the provider
brief excludes executor changes. A1/A2 and overlapping API migrations are not.

## Owner decision

On 2026-08-14 the owner selected the full ten-task 3c2 plan and declined this
document's proposed split. A1 is closure-approved and retained at `5cbeea1e` but
not integrated. A2 is the next sequential task from that exact input; no A2
implementation run was started by this record. Continue A2, A3, A4, and B-G
under the salvage plan's per-task gates and review caps. Landing A1 or A1+A2 and
then pausing remains rejected as a non-delivering island. This decision does not
authorize a provider turn, production activation, release, deployment, or
running-operator mutation.
