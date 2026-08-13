# R2f1b 3c2 salvage redesign — dual-lens adjudication

Date: 2026-08-12.

Frozen feature artifact:
`530992b7ff1e8e9151fb2a69e86f3ff71c44f905`.

Landed base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`.

Planning input: `d698e6f02f3229da3787dbc2a8630c03cb8b25df`.

## Verdict

**SALVAGEABLE; TARGETED REQUEST-ADAPTER REDESIGN APPROVED.**

The parked artifact does not need to be scrapped. The identity, global turn
authority, stale-scope fence, cancellation, HTTP lifecycle, and
acceptance-aware failure work remain useful. The request adaptation of the
shared retained-flight registry is the unsound boundary. Replace that boundary
with a request-specific journal/state machine; leave the landed process and
container flight core alone except to remove request-only additions from the
preserved feature delta.

No implementation, fold, push, production activation, provider smoke,
compatibility run, or 3d work is authorized by this verdict.

## Method, cap, and execution record

The orchestration reused the method that converged slices 3s through 3c1:

1. freeze exact base/artifact identities and preserve the rejected artifact;
2. declare one independent dual-lens design round before dispatch;
3. keep both lenses hard read-only and give them the same eight confirmed
   repaired-tail findings;
4. treat lens output as advisory and operator-check disagreements against
   production callers, persistence, wrappers, cleanup, and projection;
5. synthesize compile-correct tasks with red-first regressions, ripple lists,
   whole-tree gates, stop thresholds, and an explicit future review cap;
6. persist the inputs, original byte hashes plus repository-normalized output
   copies, adjudication, roadmap cursor, and lane handoff before implementation.

Declared design cap: one completed pass per lens, no automatic retry after a
prompt may have started. Both counted passes completed; no extension was used.

| Lens | Execution / attempt | Result | Durable artifact |
|---|---|---|---|
| Codex `gpt-5.6-sol` / xhigh / hard read-only | `exec-18c5e7fca6d011e011d220844a3eb217` / `attempt-a258ed081ae81208fa49d88621d53758` | `DESIGN LENS: READY`, no unresolved blocker | original terminal artifact 35,288 bytes, SHA-256 `04e632149391e59a180e6048f121f6a46c4168508900655c8794f1aee467e61f`; repository copy normalizes one Markdown hard break and one excess EOF blank: 35,285 bytes, SHA-256 `c32e40b8f9345c9f2a27e41fd9da4c6238a6c80af54d4516bada744e5f55ecda` |
| Claude Fable `opus[1m]` / xhigh / plan | `exec-9ebff48bc2b00f7a7a4216386fef0dce` / `attempt-e7377bb4e7a945780de18a586a7e550b` | `DESIGN LENS: READY`, no unresolved blocker | original terminal artifact 15,206 bytes, SHA-256 `ead99af9d6f4095f43f8af4723218437818eed559e14d5e70c942404e5eefa9e`; repository copy has one normalized terminal newline and SHA-256 `a2971ddadbe0852ec2c503c907efa4bf63a329160bb093bf792408b652494497` |

The common brief is 5,778 bytes with SHA-256
`732aabe3c39d9bddc07303f344bd77265215963b6e6d9964f159080323e8c6b6`.
The candidate release binary used for validation/dispatch was built from exact
main `42249b3d`, 34,090,224 bytes, SHA-256
`18adb745020fc3a95ed210e81969670d89d5f0c20b4a3e5e02cc3e3083166168`.
Both configs validated and both doctors were green before dispatch.

Two prompt-template/max-cutoff attempts refused before agent creation; they
contained no agent or prompt evidence and do not count as lens passes. The Sol
model probe initially failed only in the managed sandbox because Codex could
not initialize its local SQLite state. The exact approved-host control
succeeded, separating sandbox write denial from auth/model failure.

Fable plan mode wrote an unrequested host-side plan despite the hard read-only
prompt. That custody deviation did not touch either repository checkout. The
exact 39,974-byte artifact is preserved as
`2026-08-12-r2f1b-3c2-redesign-opus-plan-artifact.md`, SHA-256
`480fafd2de578ec8103b881f0de34f940feb10190841c49405b7c112f73a9064`.

LSP/structural navigation skills were selected for exact reference and call
coverage, but their MCP tools were unavailable in this session. Operator
adjudication therefore used bounded symbol search plus direct definition,
caller, persistence, and wrapper reads. That limitation is recorded rather
than silently upgraded to semantic-tool proof.

## Source-grounded rulings

### A1 — preserve the artifact; replace only the request adapter

Both lenses found a salvage path. Opus extended the shared retained-flight
mechanism; Sol isolated requests in a new module. Source favors Sol's boundary:

- the shared core is already consumed by landed process and container slices;
- request recovery, bounded high-frequency retirement, first-poll acceptance,
  and acknowledged publication are request-specific;
- `ApiConfig.resource_flight_route_v3` remains `None` in production, so the
  adapter has no compatibility writer to migrate;
- removing request-only branches from the feature delta reduces, rather than
  increases, the accepted process/container blast radius.

This is a bounded replacement inside the preserved artifact, not a restart.

### A2 — use a request-specific atomic child, not generic reservation plus rows

The new request journal makes one complete initial child the reservation. It
records authority, canonical request identity, and owner atomically. Private
temporary bytes have no authority and are safely cleaned on reopen. This
eliminates the zero-row gap rather than adding another rollback proof to the
generic journal. The active-child census is capped at 4,096 before mutation,
and acknowledged children retire by exact-child unlink plus root sync.

### A3 — recovery only behind positive quiescence

Recovery occurs once in `RemoteRequestAttemptV3::open_recovered`, after an
exclusive lifetime attempt lock and before route publication. Admission never
invokes recovery. Same-process serialization and the kernel lease prove that a
recovery census cannot observe a live request's reservation/create window.
Failure to recover or drain publication debt means no route and no provider
admission.

### A4 — keep exactly-once observable effect; require idempotent ack

Opus proposed terminal -> publish -> marker and an explicit at-least-once
delivery weakening. That still permits two observable consumer effects after a
sink commit/local-marker crash unless slice 5 supplies idempotence. Sol's
outbox/ack design states that requirement at the producer boundary and refuses
a no-op sink. The operator selects it: calls may repeat, but the durable sink
must deduplicate the exact delivery ID and acknowledge it. No observable
exactly-once promise is weakened.

### A5 — pre-first-poll crash is Failed; post-arm crash is Unknown/accepted

The current artifact journals `DispatchStarted` before constructing/installing
the send future, so its Unknown projection is conservative but imprecise. The
replacement wraps the actual future and durably appends `ProviderSendArmed`
immediately before its first poll. That creates a discriminating proof:
`Reserved`, `IntentJournaled`, and `DispatchAuthorized` recover as `Failed` with
accepted=false; `ProviderSendArmed` recovers as `Unknown` with accepted=true.
Recovery never resends.

### A6 — descriptor identity is request-root authority

The feature's `FileResourceFlightJournal::open` retains only root/lock paths,
reopens the lock by name, and performs later path joins. Root substitution can
redirect authority. The new request journal uses `PinnedDirectoryV1`-based
parent/root custody for every child operation and retains the attempt lock's
open fd. It refuses removal/replacement and never recreates a missing root.
The accepted shared generation journal is not widened in this redesign.

### A7 — request cleanup uses an owned async cell; generic blocking join stays

Opus proposed a deadline-aware `Condvar::wait_timeout`; Sol removed the request
path's blocking waiter. The operator selects the narrower hybrid: requests use
an async watch-backed cleanup cell and no `spawn_blocking(join_blocking)`;
generic process cleanup keeps its existing blocking join. A request drop first
transfers settlement authority, acceptance, observer, deadline, and refusal
debt to an API-owned custodian. Session removal cannot erase that cell.

### A8 — exact cleanup disposition must reach retry

Direct source confirms `cleanup_cold_session` records an
`Ok(BackendCleanupDispositionV1)` and then maps every value to `Ok(())`.
Transient retry sites use `.is_ok()`, so `Unknown`, `Retained`, or `Preserved`
can authorize another provider attempt. The final repair task returns the exact
disposition and permits retry only on `Complete`. This is not optional wrapper
cleanup; it is part of 3c2's effect authority.

### A9 — preserve the two-field cleanup carry-forward

3c2 does not arm production V3 or wrap `ContainerRw`. It adds regression
shields only. The later slice that first does either must carry inner resource
disposition and checkout disposition separately through persistence and
terminal projection. Only `Complete + Complete` may become `Complete`.

## Closure mapping for the eight repaired-tail WRONGs

| Finding | Design closure |
|---|---|
| T1 live request terminalized by successor recovery | lifetime attempt lease plus recovery-before-route; admission never recovers |
| T2 cleanup `None`/absence collapses unresolved state | cleanup cell installed before admission leaves the session lock; Legacy overlap and unresolved V3 project `Unknown` |
| T3 drop settlement refusal erased | transfer to custodian before clear; refusal/accepted/observer debt remains owned and crash prefix remains recoverable |
| T4 timeout leaves a blocking waiter | request observation is async and deadline-bound; no request-path blocking thread |
| T5 pre-intent prefixes strand | atomic initial row plus complete prefix recovery table |
| T6 4,097th request bricks census | admission cap on active children before mutation; ack retirement allows unlimited sequential requests |
| T7 terminal/publication crash gap | durable terminal outbox, idempotent delivery ID, matching durable ack, then retirement |
| T8 root replacement redirects journal | pinned parent/root identity and descriptor-relative operations throughout |

All eight have a constructible red regression in the binding plan. None
requires discarding the repaired identity/cancellation work, changing a landed
process/container contract, or arming production.

## Binding implementation cut and future cap

The binding plan is
[`2026-08-12-r2f1b-3c2-salvage-redesign.md`](../plans/2026-08-12-r2f1b-3c2-salvage-redesign.md).
Its seven sequential tasks are:

1. descriptor primitives/root custody;
2. request journal/admission/retirement;
3. attempt lease/recovery/outbox acknowledgement;
4. owned request driver/bounded observation;
5. API cleanup cell/exact projection;
6. HTTP migration and removal of the shared-flight request adapter;
7. protective-disposition consumers and reconciliation shields.

Every task has an exact predecessor commit, red-first tests, a 350-500
production-line and 700-900 total-line stop threshold, focused plus full gates,
and one commit. Per-task review cap is one independent review, followed only by
one targeted repair and one closure pass for a closed enumerable rejection.
At the cap, open-class or repeating findings park. Final aggregation gets one
Sol/xhigh correctness lens and one Fable/Opus xhigh release/compatibility lens,
with no automatic retry after prompt start.

## Remaining risk and unsalvageability threshold

- SMELL/DEFER: descriptor-relative synchronous filesystem I/O can affect Tokio
  latency. Measure before production arming; no wrong result is yet shown.
- SMELL/DEFER: the shared process/container void publisher still has its own
  terminal/callback crash gap. The request path stops using it. Its activation
  owner must not infer closure from this design.
- SMELL/DEFER: a permanently refusing idempotent sink intentionally holds the
  request window closed. That is protective unavailability; operator retry
  tooling is a later authorization.

Scrapping becomes eligible only if implementation proves that the first-poll
boundary cannot be expressed, required descriptor identity is unavailable on a
supported production host, the eventual durable sink cannot deduplicate and
the owner refuses a weaker contract, separation requires invasive landed
process/container changes, or two consecutive task cuts exceed their declared
stop thresholds because the authority is inseparable. Current source evidence
establishes none of those conditions.

## Planning-record verification

Exact checkpoint `0d72415a1f826408891d9fe64b2ca5ceb2037adf` passed:

- `git diff --check` and `cargo fmt --all -- --check` — exit 0;
- `cargo test --workspace --locked --quiet` — 3,211 passed, 0 failed,
  12 ignored across 85 harnesses.

Those totals describe the older planning branch's code, not current main or the
3c2 feature tree. The ignored population is the declared authenticated/live
set, including Kiro and local Ollama. Repository hygiene compiled and ran, then
refused only four pre-existing user-owned untracked files under `examples/`:
`a2a-bridge.2c2-repair-impl.toml`,
`a2a-bridge.m4-slice3a-impl-openegress.toml`,
`a2a-bridge.m4-slice3a-impl.toml`, and
`a2a-bridge.r2f1b-impl.toml`. They were preserved and excluded, not deleted or
rebaselined. No code, provider, smoke, compatibility, deployment, or operator
effect was exercised by these gates.

**DESIGN ADJUDICATION: APPROVE SALVAGE PLAN; KEEP 3c2 PARKED FROM IMPLEMENTATION
LANDING UNTIL TASKS A-G AND THE AGGREGATE REVIEW/GATES COMPLETE.**
