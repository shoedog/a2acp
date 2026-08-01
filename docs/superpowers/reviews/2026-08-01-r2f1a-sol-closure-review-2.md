I’ll enforce the frozen-review stop condition first. Review-round cap is one: I’ll verify cwd, clean status, HEAD/parent, and artifact digest before reading or analyzing any review material.Identity and custody match exactly, including a clean worktree and the requested SHA-256. I’m now using the repository-mandated `a2a-bridge-operator` skill for its review constraints, then I’ll read the four named documents completely before checking only the source seams needed for closure.The operator runbook does not change the review scope; it reinforces exact identity, source-to-serving causality, and a single hard read-only Sol/xhigh pass. I’m reading the repaired checkpoint line-by-line now, with the four inherited mechanisms as the only defect population unless a repair itself introduces a concrete contradiction.The focused document’s repaired mechanisms are now fully read. The key remaining questions are not stylistic: I’m checking the claimed single-read registry seam, the canonical diagnostic bounds, SQLite’s actual allocation/accounting seam, the offline barrier path, and the roadmap’s authority as the sole current cursor. No build, test, network, provider, or mutation is being used.The identity gate remains satisfied. The main closure pressure is now concentrated at two seams: provider identity across hot reloads, and SQLite’s actual WITHOUT ROWID/overflow-page behavior; I’m grounding both directly in the checked-in source before classifying them.The SQLite source falsifies one repaired invariant, not just its wording: a WITHOUT ROWID primary key still appears in `PRAGMA index_list`, and for an accepted 1 MiB node ID at 512-byte pages the proposed per-row formula is short by at least 14 overflow pages. I’m checking whether the surrounding aggregate charge independently masks that undercount or whether the closure contract itself remains impossible.The offline barrier’s core four-result mechanism is sound, but its own fail-first matrix contradicts it: “each” current `LedgerUnavailableReason` includes `Collision`, while the repaired rules require collisions to become `PrimaryFailed` and forbid targeted policy cancellation. I’m completing the final consistency pass and rechecking custody before the verdict.## Identity and custody

PASS. Verified twice, including immediately before verdict:

- cwd: `/private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo`
- checkout: clean, including untracked files
- HEAD: `ff62b1030f1c611a58e4b75aadb5c3b468b7eb9d`
- parent: `29f84447ccca6f1172b04cf44178f9429a77ebb3`
- focused artifact: `docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md`
- artifact SHA-256: `3e1a959514f12ba6d09892f5ca5a7cd56bcb841602385e917a061d4c94deb28b`

## Inherited closure

| Item | Result | Mechanism-level disposition |
|---|---|---|
| 1. Provider-attempt identity | **OPEN** | The repair freezes selection fields and consumes one immutable entry, but does not bind provider-effect fields such as `kind`, `base_url`, `api_key_env`, command/args, sandbox, auth, MCP, or cwd. A tuple-identical hot reload can therefore change the provider call while passing the digest. |
| 2. Encoded terminal reserve | **CLOSED** | The sanitizer drops every control except tab and truncates on UTF-8 boundaries; the serializer escapes only quote, backslash, and tab. Thus every sanitized string expands by at most 2×. The 1,941-byte derivation conservatively covers the current enum/code/ID bounds inside 2,048 bytes, including a 512-byte cause consisting entirely of escapable bytes. See deferred W5 for the separate invariant-violation fallback. |
| 3. Arbitrary-node-ID accounting | **OPEN** | `WITHOUT ROWID` removes the separate physical PK B-tree, but the prescribed `PRAGMA index_list` assertion is false for that schema, and the page formula undercharges sufficiently long IDs because overflow pages carry only `usable_size - 4` bytes. |
| 4. Healthy offline barrier | **OPEN** | The four-result transaction and crash-ordering mechanism is otherwise implementable, but its acceptance matrix says every `LedgerUnavailableReason`, including current `Collision`, returns `OfflineTelemetryUnavailable`; the normative rule requires collision to return `PrimaryFailed`. |

## WRONG findings

### W1 — BLOCKER: provider identity does not cover the provider call

Constructible state: freeze an API agent using endpoint A, then hot-reload only `base_url` to endpoint B before a queued node binds. The frozen tuple is unchanged, so the digest passes; the repaired path validates and consumes the new `AgentEntry`, invoking B under identity frozen before that change. The same defect applies to `api_key_env`, `kind`, command/args, sandbox, auth, MCP and session-location fields.

Mechanism and location: the asserted complete tuple contains only agent, preflight, model/fallbacks, effort and mode at [focused boundary:438](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:438>) and explicitly accepts tuple-identical edits at line 647. Current `AgentEntry` contains the omitted effect fields at [domain.rs:121](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/crates/bridge-core/src/domain.rs:121>); the live watcher applies edits at [main.rs:7932](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/bin/a2a-bridge/src/main.rs:7932>), and API backend construction consumes `base_url` and `api_key_env` at [main.rs:1219](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/bin/a2a-bridge/src/main.rs:1219>).

Conditions/likelihood: **plausible**—hot reload is a production path; the timing requires a queued, retried, or resumed attempt. Exposure includes every workflow provider surface, with provenance, credential, billing, and endpoint-custody impact.

Bounded fix: persist a canonical provider-effect digest covering every entry field that can alter backend selection, spawn, checkout, configuration, credentials, session mint, or prompt transport. Validate that digest at bind, and make the use token globally monotonic or slot-identity-bound to prevent replacement/ABA.

Cost/blast radius: **medium**, across run-spec persistence, registry port/implementation, executor binding, replay, and fixtures.

Fail-first regression: freeze API endpoint A/env X; reload B/Y before bind; require `configuration_drift` with zero resolve/configure/prompt effects. Repeat for every effect field and for slot replacement between bind and use.

Disposition: **BLOCKER**—plausible reachability × high provenance/custody impact outweighs the bounded medium repair.

### W2 — BLOCKER: the required WITHOUT ROWID schema test always rejects the intended schema

Constructible state: create exactly the proposed table. SQLite exposes its main primary-key B-tree through `PRAGMA index_list` with origin `pk`; it is not empty, even though there is no separate secondary B-tree. SQLite’s bundled implementation explicitly notes that `PRAGMA index_list` includes the WITHOUT ROWID main PK B-tree.

The incorrect empty assertion appears at [focused boundary:936](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:936>) and again in the mandatory regression at line 1333. A conforming implementation must therefore fail its schema-admission gate.

Conditions/likelihood: **common**—it arises for every migrated or newly created V2 table. Exposure is migration and all persistence acceptance; impact is an unimplementable gate rather than established data loss.

Bounded fix: require exactly one `origin='pk'` entry representing the WITHOUT ROWID table, no additional index entries, and no separate `sqlite_schema` index root for that PK.

Cost/blast radius: **low**, limited to the schema invariant and its tests.

Fail-first regression: create the table, assert one PK metadata entry, no separately rooted PK index, and no secondary entries; add a secondary index and prove rejection.

Disposition: **BLOCKER**—certain occurrence × total admission failure × low repair cost.

### W3 — BLOCKER: the arbitrary-ID page formula is not a conservative physical bound

Constructible state: use a supported 512-byte SQLite page and a 1,048,576-byte node ID. The plan’s payload is 1,050,880 bytes, so its formula charges `ceil(1,050,880 / 512) + 2 = 2,055` pages. SQLite index B-trees have `maxLocal = 102` at that page size and overflow pages carry 508 bytes. The node ID plus the 2,048-byte terminal alone consequently requires at least `ceil((1,050,624 - 102) / 508) = 2,068` overflow pages—thirteen pages more than the entire stated charge, before charging a leaf or split.

The false formula and “4n is below one page” lemma are at [focused boundary:987](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:987>) and lines 1021–1025. This also falsifies the required measured-growth assertion at line 1340. The additional logical `attempt_charge` may mask a 128-MiB breach in this particular example, but it does not make the asserted per-row bound or mandatory regression true.

Conditions/likelihood: **rare** for a 1 MiB ID, but constructible because NodeId has deliberately unlimited length and 512-byte databases are supported. Smaller thresholds arise with reserved page bytes. Exposure includes platform admission, migration, retention accounting, and future changes that rely on the stated bound.

Bounded fix: derive from actual usable page size, SQLite’s local-payload rule, and `ceil((payload-local)/(usable_size-4))`; separately cover leaf/interior splits, autovacuum pointer-map pages, and record varints, or materialize within a rollback-capable transaction and gate measured page growth before commit.

Cost/blast radius: **medium**, limited to storage accounting/migration and boundary fixtures.

Fail-first regression: a fresh 512-byte-page database with a 1 MiB ID must show calculated charge ≥ measured main-file growth; repeat around local/overflow boundaries, with reserved bytes, autovacuum, near-cap admission/refusal, WAL, rollback journal, and mixed V1/V2 migration.

Disposition: **BLOCKER**—the inherited proof obligation is false and its acceptance test cannot pass; repair remains bounded.

### W4 — BLOCKER: collision is simultaneously fail-open and fail-closed

Constructible state: seed a different trigger or terminal under the same identity, then run the offline barrier. Current source represents this as `LedgerUnavailableReason::Collision` at [workflow_history.rs:34](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/crates/bridge-core/src/workflow_history.rs:34>) and already maps terminal conflicts to it at [main.rs:4271](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/bin/a2a-bridge/src/main.rs:4271>).

The repaired normative rule correctly requires `PrimaryFailed`, no targeted policy action, at [focused boundary:724](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:724>) and lines 740–743. But the mandatory regression says a commit failing with **each** current bounded reason returns `OfflineTelemetryUnavailable` and authorizes action at lines 1295–1298. Both results cannot be implemented.

Conditions/likelihood: **rare**—requires conflicting replay, duplicated identity, concurrent writer, or corruption. Exposure includes offline and in-memory-primary workflows; impact is cancellation whose durable trigger identity was rejected.

Bounded fix: define an exhaustive classifier over the enum: genuine availability failures map fail-open; `Collision` maps only to `PrimaryFailed`. Make new variants fail compilation until classified.

Cost/blast radius: **low**, involving the contract, classifier, and barrier matrix.

Fail-first regression: seed conflicting bytes; require `PrimaryFailed`, no targeted sibling token, and global drain. Table-test every other enum variant as explicitly fail-open or fail-closed.

Disposition: **BLOCKER**—rare likelihood but high causal-custody impact and trivial bounded repair.

### W5 — DEFER: the overflow fallback deliberately discards the deepest cause

Constructible state: use the fault injection required by the plan to force the first encoding over 2,048 bytes. `minimal_over_bound` drops `deepest_cause` and replaces the code at [focused boundary:403](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:403>), while the owner design requires failed roots and strict/degraded results to retain the deepest bounded cause.

Conditions/likelihood: **theoretical** in current production: the preceding sanitizer and 1,941-byte proof establish that no valid current input reaches this fallback. Exposure is invariant-violation/future-schema diagnostic evidence; impact is loss of the most useful failure evidence.

Bounded fix: retain the deepest UTF-8 suffix that fits the remaining encoded budget, preserve the original failure class, and carry overflow as a separate static code/flag.

Cost/blast radius: **low**, serializer and projection fixtures only.

Fail-first regression: inject overflow and require `<= 2,048` bytes while preserving failure class, a nonempty deepest suffix, sticky acceptance, ancestry, and trigger identity.

Disposition: **DEFER**—theoretical reachability × evidence-only impact does not block the valid-input reserve closure, despite the low repair cost.

### W6 — BLOCKER: the sole program cursor authorizes the wrong next action

Constructible state: a new session follows the roadmap as instructed. It reads “freeze a focused R2f1a boundary, then implement” at [roadmap:866](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/docs/reliability-execution-roadmap.md:866>) and the same freeze instruction at line 967. The focused boundary already exists, is awaiting closure, and explicitly forbids implementation at [focused boundary:3](</private/tmp/a2a-bridge-r2f1a-opus.CMgins/repo/docs/superpowers/plans/2026-08-01-r2f1a-focused-boundary.md:3>) and lines 13–15, 71–74.

Conditions/likelihood: **common**—the roadmap says every new session starts there and that it is the sole volatile status cursor. Exposure is workflow authorization and provider/implementation custody; impact is a duplicate design cycle or implementation before closure.

Bounded fix: reconcile every R2f1a cursor occurrence to this rejection, link the focused checkpoint, state that implementation remains unauthorized, and record that the authorized repair/review cap is exhausted pending owner escalation.

Cost/blast radius: **low**, roadmap-only.

Fail-first regression: a cursor consistency check must find the focused artifact and current review state and reject stale “freeze R2f1a” or direct “then implement” next actions.

Disposition: **BLOCKER**—common reachability × high authorization impact × minimal repair.

## SMELL findings

None.

## Readiness and evidence

The remaining defect population is **closed-enumerable**, not open-class: provider-effect identity, two SQLite predicates, collision classification, overflow evidence retention, and roadmap reconciliation each have bounded repairs. The authorized single closure round is now exhausted, so the checkpoint must be parked and escalated rather than silently granted another repair/review cycle.

Exercised: exact identity/custody, complete focused artifact, normative owner design, parent plan, current roadmap cursor, current registry/executor/diagnostic/history seams, bundled SQLite implementation, and the page-bound arithmetic.

Not exercised, per the read-only boundary: builds, tests, migrations, runtime SQLite probes, fault injection, provider/network activity, live operator behavior, releases, or deployment.

VERDICT: REJECT
SUMMARY: Five blockers remain: incomplete provider-effect identity, two false SQLite invariants, collision’s contradictory barrier result, and a stale authoritative roadmap cursor.
