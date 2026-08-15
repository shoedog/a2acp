I’m using the configured Prism and LSP navigation instructions because this review requires a production caller and type-resolved blast-radius inventory. I’ll keep the repository work strictly read-only, make one capped review pass, and treat the supplied gates as corroboration rather than proof.The Prism/LSP navigation services are not callable in this session, so their instructions route this pass to targeted read-only Git/search inventories. The review-round cap is one pass, as requested; I will not dispatch or enter a repair loop.Identity matches the requested clean head. The cumulative range is materially larger than the final repair (+1,026/−534 across the two Rust files and handoff), so I’m reading the entire cumulative diff rather than limiting the review to the final 68-line repair, then I’ll trace only the executable seams implicated by the change.The cumulative diff is now read end-to-end. The structural fallback inventory finds no non-test caller of `JournalRootCustodyV2`, `JournalRootOperationV2`, or `NamespaceTransactionV2`; I’m now verifying that result against the module exports, API configuration default, and the complete production bodies rather than treating name-search alone as proof.The prior repair is not fully closed: `publish` still performs fallible staging-name derivation before the census. A valid 244–255-byte `ChildNameV2` therefore provides a concrete bypass of residue-first classification. I’m checking the exact error projection and the surrounding regression-test sensitivity now, then I’ll finish the required blocker/defer and confidence assessment.The closure is not complete: prior WRONG 1 is only PARTIAL, with one constructible residue-classification bypass.

## WRONG findings

### WRONG 1 — BLOCKER — Prior WRONG 1 is PARTIAL

A valid 244-byte target—such as `.a2a-v2-` plus 236 bytes—is accepted by `ChildNameV2`, whose limit is 255 bytes ([fs_custody.rs:185](/Users/wesleyjinks/code/.a2a-implement/impl-25502-s3b2uf5v/crates/bridge-core/src/fs_custody.rs:185)). On a fresh handle over a root containing any reserved residue, calling `publish` with that target returns ordinary `Refused("reserved child name: invalid child name")`, not `ProtectiveDebt`.

The mechanism is [fs_custody.rs:1247](/Users/wesleyjinks/code/.a2a-implement/impl-25502-s3b2uf5v/crates/bridge-core/src/fs_custody.rs:1247): `publish_with` derives the staging name before `guard`. Adding the 12-byte staging prefix exceeds 255 bytes, so derivation fails before the census at lines 1257–1258. On a clean root, the same reserved target also returns the wrong refusal reason instead of `Refused("reserved target")`.

Short-name `stage` and `append` are fixed, and short-name `publish` correctly stops whitelisting derived staging residue. Therefore the prior WRONG is PARTIAL rather than OPEN.

- Trigger: a 244–255-byte valid journal target plus crash/restart residue and a fresh custody handle.
- Likelihood: `rare`; no production caller exists today, but the public API accepts the input and future Task B integration can call it directly.
- Exposure/impact: no currently served run because V3 remains unarmed; after activation, a protective filesystem state can be downgraded to caller refusal, allowing recovery to be skipped and residue to remain stranded.
- Fix: when staging-name derivation fails, run `guard(None, 0, label)` first, then apply reserved-target refusal, and only then return the derivation error. Keep `Some(&staging)` only for successfully derived, non-reserved targets. Cost is small and isolated to this method and tests.
- Red regression: use a fresh handle, plant `.a2a-v2-x`, call `publish` with a 244-byte target, and require `ProtectiveDebt`, recorded debt, unchanged bytes, and count. Add a clean-root long reserved case requiring exactly `Refused("reserved target")`. The first case fails on `863f2fd4`.

## SMELL findings

### SMELL 1 — DEFER — Compile-only initial owned-surface red evidence

The recorded A4 surface red was ten missing-API compile errors, not an executed behavioral failure ([handoff:499](/Users/wesleyjinks/code/.a2a-implement/impl-25502-s3b2uf5v/docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md:499)).

- Trigger/likelihood: `plausible` when later maintainers rely on historical fail-first provenance.
- Exposure/impact: evidence quality for reviewers; no demonstrated runtime error.
- Fix/test: retain mutation-control evidence showing each executable post-change assertion fails when its corresponding outcome/effect is degraded. Low test-only cost.
- Disposition: `DEFER`; this cannot retroactively create an executable pre-API base and establishes no current wrong output.

### SMELL 2 — DEFER — Missing replace 4,094 positive boundary

The capacity test still covers replace only at 4,095 and 4,096 children ([namespace_transaction.rs:1112](/Users/wesleyjinks/code/.a2a-implement/impl-25502-s3b2uf5v/crates/bridge-core/src/namespace_transaction.rs:1112)). The mechanism’s `names + footprint > 4096` comparison admits 4,094 plus two entries, so no wrong behavior is demonstrated.

- Trigger/likelihood: `rare`; a future activated journal exactly at 4,094 entries.
- Exposure/impact: a boundary regression could cause false protective refusal or overcommit.
- Fix/test: add a successful 4,094 replace case asserting successor bytes, empty residue, clear debt, and bounded count; mutation from `>` to `>=` must make it red. Under one hour, test-only.
- Disposition: `DEFER`.

### SMELL 3 — DEFER — Append’s repair regression is not independently fail-first

The object-present test constructs an array with `stage` first ([fs_custody.rs:5319](/Users/wesleyjinks/code/.a2a-implement/impl-25502-s3b2uf5v/crates/bridge-core/src/fs_custody.rs:5319)). `stage` records debt, so later `publish` and `append` exit through `refuse_debt`; reverting only append’s census/refusal order would still pass this test. Publish has the separate derived-staging regression, but append has no fresh-handle repair test.

- Trigger/likelihood: `plausible` during a later append refactor.
- Exposure/impact: future residue-bearing append requests; a regression could again misclassify protective state.
- Fix/test: give each mutator its own fresh case/handle and verify that restoring append’s pre-repair order fails. Under one hour, test-only.
- Disposition: `DEFER`; current append control flow is correct.

## Evidence assessment

- Exact head `863f2fd4840479c15efbfc0622d8d25291c4d1f0`, requested branch, and clean worktree were verified at entry and exit.
- The entire 1,942-line cumulative diff and handoff were read. Prism/LSP services were unavailable; repository-wide read-only search found no non-test callers, persistence encoding, or served projection for the new custody/transaction APIs.
- Production still explicitly assigns `resource_flight_route_v3 = None` ([main.rs:1622](/Users/wesleyjinks/code/.a2a-implement/impl-25502-s3b2uf5v/bin/a2a-bridge/src/main.rs:1622)).
- Recorded in-memory debt still dominates all mutators; capacity accounting and recovery’s empty-census → root-sync → route-proof → clear sequence are unchanged. Short clean-root reserved requests retain no-effect/refusal semantics. The blocker is specifically fresh-handle residue domination before fallible publish name derivation.
- No builds or tests were run under the read-only contract. The supplied 610/0 and 4,024/0 gates are corroboration only and do not cover the 244-byte case.
- Confidence: **97/100**. Executing the proposed exact-head regression would raise it. An enforced ≤243-byte publish precondition would lower exposure. The conclusion would collapse only with mechanism-level proof that 244–255-byte `ChildNameV2` values cannot reach `publish` or that the contract intentionally permits derivation refusal to precede residue census; the current type and stated invariant show neither.

VERDICT: REJECT
SUMMARY: Prior WRONG 1 is PARTIAL because long valid publish targets still bypass residue-first census; three coverage smells remain deferred.