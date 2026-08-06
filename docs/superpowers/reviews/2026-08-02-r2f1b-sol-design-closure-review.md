# R2f1b Sol/xhigh design closure review — historical verdict

> Evidence custody: the complete bridge terminal artifact is SHA-256
> `23fc61c2f5f77f5eda4951e025608884ff299552ab0bb9483a0f8037ac3d3541`.
> The finding text below is preserved from its first heading onward; only ephemeral link targets were normalized. It reviewed draft SHA-256
> `ea7dfc69fd68b830b5a3b6343b50a9770dc264b0a8566e5d6b6411f41c0dbfbe`, checked in as [the reviewed draft](2026-08-02-r2f1b-reviewed-draft.md), not the repaired
> checked-in boundary. The five WRONG findings and both SMELL corrections were folded once into
> `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md`. No second design review was run,
> honoring the declared cap.

# R2f1b closure review

REJECT. Exactly one hard read-only review was performed. The frozen checkout and design remained unchanged through the final recheck.

## Evidence custody

Measured twice, initially and immediately before verdict:

- cwd: `/private/tmp/a2a-bridge-r2f1b.1OMn7B/worktree`
- tracked and full status: clean
- HEAD: `56334e98291c96c69f5a6fc37a15a8fdaf9634e0`
- tree: `17278a20f0d8e450784510c68f103d1c6c1041d4`
- parent: `bacb5036120578e09ecb6ec32abffc416b86d381`
- design SHA-256: `ea7dfc69fd68b830b5a3b6343b50a9770dc264b0a8566e5d6b6411f41c0dbfbe`

The supplied terminal-synthesis SHA had no associated file path, so it remains supplied, unmeasured evidence. The supplied 3,192/0/12 test result was not rerun. No file was edited; no build, test, provider, delegation, or operator action occurred.

All named normative sources, AGENTS.md, the operator skill’s required references, and the entire design were read. R2f1b is specified-but-absent at this HEAD; source was used as current fact and feasibility evidence.

## WRONG findings

### WRONG 1 — Node-local success can delete work that must survive a later workflow cancellation — BLOCKER

Constructible state: two parallel V3 worktree roots run under `bounded_independent`. Node A finishes normally after producing useful edits; node B remains pending until the two-hour cutoff. Current execution settles node cleanup before terminal observability ([executor.rs](../../../crates/bridge-workflow/src/executor.rs)), and current worktree cleanup removes the checkout ([backend.rs](../../../crates/bridge-worktree/src/backend.rs)). The design says normal success requests automatic deletion ([design](2026-08-02-r2f1b-reviewed-draft.md)). When B later reaches the workflow cutoff, the required preservation of every materialized worktree cannot preserve A because A is already removed. That contradicts the owner invariant that worktree contents survive cancellation ([owner design](../../../docs/superpowers/specs/2026-07-20-r2f-owner-design.md)) and the design’s own cancellation flow.

- Trigger: a completed worktree node followed by any later workflow failure/cancellation.
- Likelihood: plausible and routine for parallel workflows.
- Population: all V3 workflows with multiple materialized worktree nodes.
- Impact: irreversible loss of useful edits and no R2f2 recovery path.
- Confidence: high. It rises with the current node-cleanup ordering; it falls only if “normal success” is explicitly redefined as global healthy workflow success. It collapses if deletion capability can be minted only after the global terminal decision.
- Fix: separate session settlement from checkout disposition. Keep completed checkouts protected until the global outcome is known; only an all-healthy workflow outcome may authorize automatic deletion, or preserve all V3 checkouts until explicit R2f2 disposition.
- Cost: medium.
- Regression: A completes, B reaches cutoff, and A’s exact checkout remains recoverable; no node-local success may call `provider.remove`.
- Class: closed/enumerable.

### WRONG 2 — Resume reuses the normative attempt identity — BLOCKER

The design retains the original `attempt_id` and adds `AttemptEpochIdV1` ([design](2026-08-02-r2f1b-reviewed-draft.md)). The owner contract instead defines `attempt_id` as unique to one monotonic attempt and requires boot resume/takeover to mint a new attempt with parent and ordinal lineage ([owner design](../../../docs/superpowers/specs/2026-07-20-r2f-owner-design.md)). Current primary resume already mints that successor identity ([ids.rs](../../../crates/bridge-core/src/ids.rs)), while the restored frozen run spec still contains its predecessor attempt.

Constructible result: after a crash, the successor monotonic execution uses the predecessor `run_spec.attempt_id` for policy/control identity while task/history uses the newly minted successor. Control-event IDs, cleanup ownership, terminal evidence, and custody records can therefore collide with or be attributed to the predecessor.

- Trigger: every V3 crash resume.
- Likelihood: certain on resume.
- Population: all resumed V3 attempts across offline, served, batch, and MCP paths.
- Impact: false attempt attribution, replay/conflict errors, and broken causal history.
- Confidence: high. It rises from the explicit “retain” rule and current split identities; it collapses only if V3 carries the new `AttemptIdentity` everywhere while retaining the old ID solely as `origin_attempt_id`.
- Fix: mint and propagate a true successor `AttemptId`, ordinal, and parent before the successor clock or effects. Keep original checkout identity separately so the exact target is reused without reusing attempt identity.
- Cost: medium.
- Regression: resume has a distinct attempt ID and parent link, preserves the same target, and derives policy/resource/terminal IDs from the successor.
- Class: closed/enumerable.

### WRONG 3 — The separate cleanup row cannot coexist truthfully with an unchanged immediate `NodeTerminalV1` primary — BLOCKER

Splitting cleanup evidence is sound in principle, but the proposed representation is contradictory. The design requires an immutable primary immediately while cleanup is `Pending`, yet says the primary keeps the unchanged `NodeTerminalV1` shape ([design](2026-08-02-r2f1b-reviewed-draft.md)). That type has a mandatory cleanup field whose only states are Complete, Failed, NotNeeded, and UnknownLegacy ([execution_policy.rs](../../../crates/bridge-core/src/execution_policy.rs)); it cannot encode Pending.

Constructible result: the failed-root state explicitly required by design §5.6 must either write a false cleanup value, fail schema encoding, or misuse UnknownLegacy. Existing projections count UnknownLegacy as partial ([detached.rs](../../../crates/bridge-coordinator/src/detached.rs)), so it is not a neutral placeholder.

- Trigger: every V3 primary known before cleanup settles, especially failure/deadline/cancel.
- Likelihood: certain by the proposed ordering.
- Population: all V3 node records and every offline/served/batch/A2A/MCP/history projection.
- Impact: false durable cleanup state, failed CAS recovery, or incompatible projections.
- Confidence: high. It falls only if an unstated V3 primary type exists; it collapses when that type and its joins are specified.
- Fix: retain V1 unchanged for V2 only. Add a versioned `NodePrimaryRecordV3` without mutable cleanup, and pre-reserve both its placeholder and a cleanup-Pending row atomically before effects. Every V3 reader joins the two rows; old readers reject or skip the new evidence schema.
- Cost: medium.
- Regression: crash after primary and before cleanup settlement reads as Primary+Pending, resumes only the cleanup CAS, then projects the final cleanup exactly once; V1 budget and deepest-cause tests remain unchanged.
- Class: closed/enumerable.

### WRONG 4 — Custody preparation is outside every finite clock — BLOCKER

The design performs root opening, all custody locks, file creation, `sync_all`, no-replace publication, and parent sync before exposing queue/work timers ([design](2026-08-02-r2f1b-reviewed-draft.md)). The reused primitive performs synchronous `sync_all` ([local_file.rs](../../../bin/a2a-bridge/src/local_file.rs)).

Constructible state: the custody filesystem stalls indefinitely during file or parent fsync. No queue, control, or work timer has been armed, so the attempt never terminalizes. This violates the global finite-ownership invariant and the explicit 30-minute queue/admission cap.

- Trigger: stalled disk, FUSE/NFS filesystem, wedged mount, or nonreturning sync.
- Likelihood: rare locally, credible under storage failure.
- Population: every V3 worktree attempt.
- Impact: the exact unbounded workflow condition R2f is intended to eliminate.
- Confidence: high. It falls if preparation is independently owned and bounded despite the written ordering; it collapses if that ownership is made explicit.
- Fix: begin a finite admission/preparation clock before the first potentially blocking operation and run custody preparation under an independently owned flight. Expiry must admit zero provider/process effects, preserve/quarantine ambiguous publication state, and return or transfer a typed preparation result. The work cutoff remains unarmed until protection completes.
- Cost: medium–high.
- Regression: a deliberately nonreturning sync reaches typed bounded terminal/ownership transfer with zero provider, session, process, or destructive sweep calls.
- Class: closed boundary repair.

### WRONG 5 — Watchdog observe-only demotion is itself an unapproved compatibility break — BLOCKER

The current operator contract says `[agents.watchdog]` cancels at idle or hard-wall expiry ([containerized-agents.md](../../../docs/containerized-agents.md)); the owner policy says a smaller command/provider/watchdog limit may shorten a phase ([owner design](../../../docs/superpowers/specs/2026-07-20-r2f-owner-design.md)). The design instead silently makes that configured watchdog actionless for V3 workflows while claiming this avoids a compatibility break ([design](2026-08-02-r2f1b-reviewed-draft.md)).

Constructible result: an unchanged agent config with a 10-minute hard wall previously returns `AgentTimedOut` around ten minutes; under V3 it continues toward the two-hour cutoff. The configured limit no longer wins.

- Trigger: any V3 workflow using an agent with `[agents.watchdog]`.
- Likelihood: plausible.
- Population: all watchdog-configured workflow agents; direct sessions remain different.
- Impact: silent extra runtime/cost and changed cancellation/collateral behavior.
- Confidence: high. It falls if V3 activation is separately explicit and acknowledges this migration; it collapses with a reviewed versioned policy choice.
- Fix: minimally refuse AutomaticR2f1b before effects when a legacy watchdog is configured. A later explicit schema may freeze the hard wall as the smaller provider bound and require operator opt-in for idle observe-only behavior. V2 and direct behavior remain unchanged.
- Cost: low design, medium implementation.
- Regression: unchanged legacy config either retains legacy behavior or receives typed pre-effect V3 refusal; no silent demotion. Also cover concurrent direct and V3 sessions on one ACP generation.
- Class: closed/enumerable; requires owner adjudication.

## SMELL findings

### SMELL 1 — Custody transitions omit unused candidates and `RecoveredLive` — DEFER

Every frozen fallback/preflight candidate receives `ProtectionPrepared`, but the normal terminal algorithm does not specify exact settlement for candidates never materialized. Separately, resume publishes `RecoveredLive`, yet that state is absent from the state machine and sweep protection list.

No incorrect deletion is demonstrated because the conservative behavior is to retain/refuse; the risk is stranded metadata and ambiguous later recovery.

- Trigger: unused fallback candidates or crash after resume claim exchange.
- Likelihood: high for unused candidates; low for the crash point.
- Population: preflight/fallback and resumed V3 runs.
- Impact: accumulating markers, reserved paths, or unnecessary operator debt.
- Confidence: medium-high; collapses with explicit transitions.
- Fix: define an exact-absence-proved unused-marker settlement that never invokes provider removal, and make `RecoveredLive` either a real protective state or an enriched `LiveProtected` state.
- Cost: low–medium.
- Regression: unused candidate marker settles after proving no target/registration; crash at claim exchange remains protected and resumable.
- Class: closed/enumerable.

### SMELL 2 — The roadmap’s authoritative cursor remains contradictory — DEFER

The focused design says Git is authority and defers reconciliation to slice 6, while the roadmap simultaneously says R2f1a is owner-authorized/in progress and parked/unauthorized ([roadmap](../../../docs/reliability-execution-roadmap.md), [roadmap](../../../docs/reliability-execution-roadmap.md)). Git proves landed source, not current implementation authority.

- Trigger: an operator using the roadmap to decide whether R2f1b implementation may begin.
- Likelihood: high.
- Population: all subsequent operators/agents.
- Impact: avoidable stop or unauthorized interpretation; no code result is demonstrated here.
- Confidence: high; collapses after one authoritative reconciliation.
- Fix: reconcile the roadmap to `56334e9`, the R2f1a closure, this verdict, and the actual next authorized action before slice 1—not in slice 6.
- Cost: low.
- Regression: documentation/status consistency gate over header, dependency graph, program table, next action, and current handoff.
- Class: closed documentation correction.

## Closed checks

- The inherited R2f1a worktree-cwd WRONG is closed at mechanism level: `validate_bound_worktree` consumes the persisted target without deriving from process run identity ([provider_path.rs](../../../crates/bridge-worktree/src/provider_path.rs)), and the wrapper checks the exact bound session cwd before inner configuration. It is not carried forward.
- One generation-scoped resource flight, admission closure before signal, retained process/container capability, and collateral fan-out match the owner contract.
- The proposed completion-first cutoff ordering and sorted ready batch satisfy scheduler tie requirements.
- Dual-pattern sweeps and non-destructive V3 handling are directionally correct, subject to the custody-state SMELL.
- V2/manual preservation, a new V3 envelope, a new sidecar name, and additive storage are appropriate rollback boundaries once the V3 primary schema is corrected.
- No open-class finding population was found.

## Minimal blocking repair set

1. Gate deletion on the global workflow outcome so completed siblings cannot be destroyed before a later cancellation.
2. Mint and propagate a real successor `AttemptId` on resume while separately retaining origin checkout identity.
3. Version the V3 primary record and atomically pre-reserve its cleanup-Pending row; define all projection joins.
4. Put custody preparation under finite pre-effect ownership without making destructive workflow cancellation reachable before protection.
5. Replace silent watchdog demotion with explicit pre-effect refusal or a separately reviewed, versioned operator opt-in.

These are closed, bounded repairs, but all five are required before implementation can start. The two SMELL corrections remain non-blocking and should be folded into the same focused artifact. No retry or second review is authorized by this verdict.

VERDICT: REJECT
