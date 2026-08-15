I’m starting the single capped closure-review pass. I’ll freeze the checkout and diff identity, read the complete B1 module and handoff material, then trace every production reference and Task A boundary using repository-only search; the navigation skill files are outside the authorized repository, so I’ll use the required read-only fallback rather than crossing the scope boundary.The checkout identity matches the requested clean head and the three-commit range exactly. I’ve also confirmed the stated 879-line module, 2-line export, and 57-line handoff delta; I’m now reading the module in bounded line-numbered sections because the first combined repository read was truncated and is not admissible as a completeness check.The six inherited repair items are now pinned from the committed task brief, not inferred from the handoff. The shipped code visibly closes the API-shape, nested-decode, duplicate-ID, overflow-classification, and needless-borrow defects; I’m tracing the harder parts now—Task A’s exact mutation semantics, crash cuts, capacity arithmetic, and whether the grammar admits constructible inconsistent roots that the tests do not cover.The causal trace is complete. The B1 code is not called by any production path, Task A and `Cargo.lock` are byte-unchanged in the range, and the authority has one construction expression. I have not found a constructible current production mis-result; the remaining issues are pre-arming coverage/invariant carry-forwards, so I’m doing the final evidence reconciliation and literal verdict formatting now.## WRONG findings

None established.

## SMELL findings

1. **SMELL — Protective-path regression coverage is incomplete.**  
   The production mappings at [remote_request_flight.rs:151](/Users/wesleyjinks/code/.a2a-implement/impl-56580-abz3axmg/crates/bridge-core/src/remote_request_flight.rs:151) are exhaustive by inspection, but the all-outcome test uses an independent test-only mapper at [remote_request_flight.rs:557](/Users/wesleyjinks/code/.a2a-implement/impl-56580-abz3axmg/crates/bridge-core/src/remote_request_flight.rs:557). Only `ProtectiveDebt` is injected at a real Task A boundary. The original B1 red was missing-symbol compilation, not behavioral fail-first evidence.

   - Trigger: B2/C changes the wire or a stage/publish/sync/replace mapping.
   - Likelihood: `plausible`; those tasks will extend this module.
   - Exposure/impact: no current production users; after V3 arming, a regression could accept corrupt state or issue authority after an uncertain mutation.
   - Fix: localized fault seams and tests for real `Refused`, `Retained`, `Unsupported`, I/O/`Unknown`, and replacement outcomes, plus child schema/digest/name/state corruption. Roughly 0.5–1 day, test-heavy and module-local. Red tests should deliberately flatten each production mapping and prove the corresponding case fails.
   - **DEFER**: current production mappings are correct by direct trace and the module is uncalled.

2. **SMELL — B1 does not locally enforce the owner constructor invariant.**  
   `admit` accepts `ResourceFlightOwnerV1` directly at [remote_request_flight.rs:404](/Users/wesleyjinks/code/.a2a-implement/impl-56580-abz3axmg/crates/bridge-core/src/remote_request_flight.rs:404), while that public type’s fields permit bypassing `new` and constructing an empty `owner_key`. Its derived decoder likewise accepts that value.

   - Trigger: a future caller uses a struct literal, or an owned root contains a syntactically valid empty owner.
   - Likelihood: `rare`; current repository callers are absent and established construction uses `new`.
   - Exposure/impact: future V3 request ownership and result routing could lack a meaningful owner key.
   - Fix: add a shared validation method and call it before mint and during census. Low cost and narrow blast radius. Red tests: empty-owner admission must mint zero IDs and preserve every root byte; an empty-owner wire must refuse without mutation.
   - **DEFER**: no reachable production caller currently constructs this state.

3. **SMELL — The sealed authority currently carries only the request ID.**  
   The persisted child binds attempt and ordinal, but [RemoteRequestAuthorityV1](/Users/wesleyjinks/code/.a2a-implement/impl-56580-abz3axmg/crates/bridge-core/src/remote_request_flight.rs:105) contains only `request_id`. A collision is detected within one journal, not across attempts.

   - Trigger: future multi-attempt consumers key cancellation/publication solely by the exposed ID and two attempts receive the same CSPRNG output.
   - Likelihood: `theoretical-only` for a healthy CSPRNG; no consumer exists yet.
   - Exposure/impact: potentially severe cross-attempt aliasing after V3 activation.
   - Fix: before consumer integration, bind private attempt and ordinal fields into the authority and all delivery/control keys. Low-to-medium cost in B2/C. Red test: force the same request ID in two journal roots and prove their full authorities and stale controls cannot alias.
   - **DEFER**: later tasks can extend the private representation before arming production.

## Evidence assessment

The six inherited findings are all **FIXED**:

1. Unforgeable authority: private field, no `Clone`/`Copy` or constructor, borrowed accessor, and one production construction expression at [remote_request_flight.rs:506](/Users/wesleyjinks/code/.a2a-implement/impl-56580-abz3axmg/crates/bridge-core/src/remote_request_flight.rs:506).
2. Strict nested attempt decoding: private remote wire with `deny_unknown_fields` at [remote_request_flight.rs:56](/Users/wesleyjinks/code/.a2a-implement/impl-56580-abz3axmg/crates/bridge-core/src/remote_request_flight.rs:56).
3. Duplicate mint: census comparison precedes staging at [remote_request_flight.rs:453](/Users/wesleyjinks/code/.a2a-implement/impl-56580-abz3axmg/crates/bridge-core/src/remote_request_flight.rs:453).
4. Enumeration overflow: exact `EnumerationLimitExceeded` maps to `Capacity` at [remote_request_flight.rs:296](/Users/wesleyjinks/code/.a2a-implement/impl-56580-abz3axmg/crates/bridge-core/src/remote_request_flight.rs:296).
5. Clippy defect: the needless borrow is removed at [remote_request_flight.rs:322](/Users/wesleyjinks/code/.a2a-implement/impl-56580-abz3axmg/crates/bridge-core/src/remote_request_flight.rs:322).
6. Cap compliance: **FIXED**. Of 879 module lines, 381 are test-only and 498 production; the two-line Unix export makes exactly 500.

The accounting WRONG is **resolved**: `+172/-38` total module repair, `+43/-38` production-region churn, 381 cumulative test-only lines, and 500 cumulative production additions are mutually consistent.

Required judgments:

- Admission atomicity passes for B1: every stable injected cut returns no authority; final request rows are complete decodable publications; authority follows child publication, checkpoint replacement, and final exact-`Complete` sync.
- Task A consumption passes at the control boundary: every protective/refused result returns a typed refusal. A Task A `Retained` result may already have changed visible bytes, but it never permits authority return and forces reopen.
- Capacity arithmetic accounts for the checkpoint, new child, and two-entry replacement footprint. The cap-8 fixture correctly admits five active children and refuses the sixth before mint.
- Strict checkpoint/child/attempt decoding and digest/name checks are present, subject to the deferred owner-validation smell.
- Scope passes: repository search finds only the Unix export and colocated tests; Task A files, `Cargo.lock`, callers, routes, persistence consumers, and V3 arming are unchanged. No `rustfmt::skip` was added.
- The B1/B2 split is valid. Current `open` does **not** close an orphan; checkpoint repair, pre-send closure, acknowledgement, retirement, and reopen self-healing remain explicitly unimplemented B2 behavior. The private wire/census structures and retained snapshots do not foreclose that work.
- Freezing the handle after a duplicate mint is sound fail-closed policy. Reopen is an explicit trust boundary after evidence that the identity source may be unhealthy.
- The flock/EBADF container classification is credible but not independently proven here: the failing harness and lock paths are untouched and the new module has no caller, while the exact head is supplied host-green. A same-container base/head control was not supplied, so I treat “environmental” as an inference rather than causal proof.

Observed in this pass: clean exact head `6033fd34`, exact three-commit ancestry, complete diff/module read, zero Task A or lockfile changes, no production references, and clean `git diff --check`. The 4,034/0/13 host suite and other gates remain supplied evidence because this review was prohibited from running them.

Confidence: **92/100**. Same-container base/head evidence and real Task A fault-injection tests would raise it. A B2 prototype exposing insufficient recovery state would lower it. Discovery of a current production caller, another authority construction path, or a behavioral protective-outcome test returning authority would collapse it.

VERDICT: APPROVE
SUMMARY: B1 and all six targeted repairs are correct and scoped; defer pre-arming fault coverage, owner validation, and full attempt-bound authority identity.