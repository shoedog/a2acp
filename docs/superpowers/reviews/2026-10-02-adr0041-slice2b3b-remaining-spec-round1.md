# ADR-0041 2B3b remaining-task spec review1/2

Controller binding: clean2d8a8fb4441f7a4d5b5377d84d5104e6d03ddccb, base251289fe.
Raw workflow output SHA256 ecc96d7aa8bcf72f22fe8b2b9369d2d7f0978cf456c39859168f4a24f4764c1c. Sol gpt-5.6-sol/xhigh, hard read-only; no execution.
Session43775 completed/cleanup complete, exec-fffd851e8bc3155755d1a37bced0d873, attempt-52c3d095d91dcc5342352ae2ce4561cd.
REJECT wrong1/smell2/blockers1 round1/2, CLOSED POPULATION. Deadline preliminary concern was withdrawn
mechanistically: wrapper carries same stack Instant. Owner chose Empty-refs pre-repository refusal.
Five consecutive remaining increments adopted before implementation; bootstrap structural scan becomes
function-scoped with direct-bootstrap behavior preserved. These refinements are pending final spec2/2.
Original A review history/checkpoint/source unchanged. Original raw output follows, including progress text.

---

I’m binding the controller checkout, hashes, and normative review chain first. I’ll then read the master, both B drafts, the full Pass A approval/source-review record, the adopted source, and the current handoff. I’m treating this as remaining-task spec review round 1 of the declared 2-round cap; I will not mutate or execute the implementation.The checkout is clean at `2d8a8fb4441f7a4d5b5377d84d5104e6d03ddccb` on `work/adr0041-2b3b-bootstrap-20261001`; the original cumulative base resolves to `251289fe3fada54a740be4879fa48e7adeb9f3cb`. Every supplied master, B1/B2, raw review, adopted-source, and handoff hash matches. The checked-in round-6 approval document is additionally bound at SHA-256 `e789aa4b…`; it was not assigned a hash in the brief, so I’m treating the locally committed bytes as the normative approval copy and will cross-check them against the raw `0217fbce…` review.The split’s stated coverage aligns with the master at a high level, but the convergence question depends on actual helper boundaries and the current private APIs. I’m now reading the adopted Rust sources end to end, then I’ll map each remaining master obligation to B1 or B2 and test whether either slice is still a big-bang implementation.A concrete custody conflict has emerged: the approved Pass A result retains the capsule, pins, ledger, and evidence, but not the caller’s original absolute `Instant`. I’m finishing the source read to determine whether the drafts repair that gap or would force B2 to accept a replacement deadline, which would violate the master. I’m also checking whether the proposed B1/B2 sizes can realistically converge within their declared two-round caps.The apparent deadline gap is not itself a contract conflict: the master’s eventual `restore_git_plane_v1(..., deadline)` can keep the same stack `Instant`, pass it into Pass A, and then pass that identical value into B1/B2 helpers. No new deadline or result field is required. I’m now checking the decoder and retained-descriptor APIs for a genuine layout mismatch and separating that from the review-cap sizing question.The source-layout check found one actual semantic hole, separate from implementation size: the already-approved Pass A accepts a valid capsule where `RefsAndHead` is `Empty` and therefore has no active frame, but B2 requires a final `HEAD` before Git closure while forbidding bootstrap `HEAD` from entering the final repository. The master defines no exact `HEAD` bytes or branch name to synthesize. I’m closing the review around that owner decision and the smallest serial repair that preserves the approved Pass A bytes.Bound to clean controller checkout `2d8a8fb4441f7a4d5b5377d84d5104e6d03ddccb` on `work/adr0041-2b3b-bootstrap-20261001`, cumulative base `251289fe3fada54a740be4879fa48e7adeb9f3cb`. Every supplied master, draft, review, source, and handoff hash matches. I performed only read-only Git/source/document inspection; supplied test receipts were not treated as local execution.

**WRONG — `RefsAndHead=Empty` has no defined final `HEAD` behavior. Blocker. Confidence 97/100.**

The state is constructible with the existing fixture: all non-object coverage classes, including `RefsAndHead`, are `Empty`, `original_refs` is empty, and no active frame exists ([fixture](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_restore_git_tests.rs:206)). Pass A deliberately accepts `(Empty, no payload)` for both active classes ([classifier](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_restore_git.rs:887)), matching the master’s “Empty needs no frame” requirement ([master](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:97)).

B2 then materializes only prevalidated frame entries, so this state creates no final `HEAD` ([B2 draft](/private/tmp/adr0041-2b3b-20260929/2b3b-pass-b2-draft.md:19)). Yet the master requires final commands only after `HEAD` exists, forbids copying bootstrap `HEAD`, and provides no deterministic branch name or exact bytes to synthesize ([master](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:165)). Refusing this state contradicts the accepted Empty-active-class contract; copying bootstrap `HEAD` is forbidden; synthesizing one changes semantics without owner authority.

Before B1 dispatch, the owner must choose and record one bounded rule:

- Require `RefsAndHead=Captured` with exactly one valid `HEAD` for complete phase-G success; make `RefsAndHead=Empty` a typed pre-repository refusal while leaving approved Pass A unchanged.
- Authorize deterministic synthesis for `RefsAndHead=Empty`, including exact bytes, branch name, provenance, and agreement semantics.

Regression: run the existing all-Empty capsule for both SHA-1 and SHA-256. The repaired contract must require either the exact authorized synthesized `HEAD` plus final closure, or the exact typed refusal with `active_materialization_began=false`, no `repository/`, and unchanged original ledger/deadline custody. Confidence collapses only if an owner-approved normative source already defines one of those outcomes.

**SMELL — both proposed slices remain too broad for a two-round source-review cap.**

B1 combines active-frame custody and decoding, aggregate path/class/collision validation, seal accessors, several ref grammars and resolution, inventory agreement, and config encoding. B2 combines two-store materialization, metadata decoding, filesystem custody, final agreement/config, every Git-child outcome, typed progress/result, and all final gates. None of the remaining B result/parser/materialization foundation exists yet, while Pass A alone added 10,514 lines and required six reviewed rounds.

Preserve the same lane and ownership, but use consecutive source increments:

1. **B1a:** retained-frame remeasurement and deadline-aware null-sink prevalidation through the trailer; aggregate class/path/type/parent/portable-collision rules.
2. **B1b:** manifest accessors, pure HEAD/loose/packed/peeled parsers, bounded resolution/agreement, and config encoder, incorporating the owner’s Empty-HEAD decision.
3. **B2a:** create-new final object hierarchy and exact second object-store copy/accounting.
4. **B2b:** second frame decode, metadata materialization, final-descriptor ref agreement, then config creation.
5. **B2c:** final Git commands with all-outcome reconciliation, typed result/progress, wrapper wiring, and deadline-paced final gates.

Each increment should bind the exact approved predecessor. At runtime B2c must still pass one original capsule, ledger, pins, and stack `Instant` through all helpers; these are source-review seams, not separate restore attempts.

**SMELL — the inherited Pass A structural guard needs an explicit transition.**

The current test scans the whole Git helper module and forbids literal `repository` and `.git` references ([test](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/custody_restore_git_tests.rs:3103)), while B2 says to preserve every A regression. A compliant placement in `custody_restore.rs` can avoid conflict, so this is not a demonstrated blocker. Before B2a, specify that the Pass A invariant remains function-scoped: direct `bootstrap_git_objects_v1` calls must never create `repository/`. If B code enters the Git helper module, narrow only the source scan’s scope and retain a behavioral Pass A regression rather than deleting or broadly weakening the guard.

Apart from the unresolved Empty-HEAD semantics, the drafts cover the remaining master obligations: B1 places full active-frame prevalidation before repository effects and supplies bounded ref/config foundations; B2 carries create-new custody, both object-store charges, descriptor revalidation, final agreement and closure, all-outcome accounting, retained result/progress, and census→digest/tree→chains→root-last gates. Existing frame, pin, and runner APIs suffice without unsafe, dependency, runner, codec, or public changes. The same original deadline can remain a single `restore_git_plane_v1` argument passed to Pass A and every later helper; it need not be added to the approved Pass A result. B2’s execution base must bind only after every preceding source increment is approved.

VERDICT: REJECT
SUMMARY: wrong=1 smell=2 blockers=1 round=1/2
CAP CLASSIFICATION: CLOSED POPULATION

