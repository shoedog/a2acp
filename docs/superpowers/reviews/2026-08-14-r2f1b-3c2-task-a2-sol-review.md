I’m using the `prism-nav` and `lsp-nav` skills because this review requires a structural and type-resolved caller inventory. I’ll keep the investigation hard read-only, bind it to `5cbeea1e..3890fa6c`, and treat the supplied gates only as corroboration. The single review-round cap is one pass, as requested.The navigation services are not callable in this session, so I’m falling back to bounded repository search and direct call-site reads, as the review contract allows. I’m first freezing the checkout identity and enumerating the exact diff before tracing the new public surface.Identity is exact and the committed worktree is clean. The diff contains only the two declared source files and the handoff: 456 added lines in custody, a one-line liveness visibility change, and 25 handoff lines. I’m now reading the full changed files, including all pre-existing surrounding mechanisms, then I’ll independently map every new symbol and regression to its production consequence.The source mechanism is additive and matches the intended acquisition order: mutex → descriptor-relative lock open → mandatory identity check → nonblocking object flock → route re-proof while held. The remaining review is focused on the race boundaries the tests actually discriminate, public-surface reachability, and whether any supplied verification claim overstates source evidence.## WRONG findings

None established.

## SMELL findings

1. `SMELL` — The handoff’s “admissible red evidence” is compile-only. [handoff.md:456](</Users/wesleyjinks/code/.a2a-implement/impl-66546-s8d4i725/docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md:456>) records missing-symbol errors, which prove the API was absent but are not behavioral fail-first evidence under this review contract.

   - Trigger: every acceptance review relying on this receipt.
   - Likelihood: `common`; this is the only recorded pre-change red.
   - Exposure/impact: reviewers and operators receive weaker regression evidence, but runtime behavior is unaffected.
   - Fix/cost: small, test-only evidence pass. Run the existing focused tests against bounded mutations that remove/move the post-flock proof, bypass lock identity, replace real flock with unconditional success, and add a root-creation fallback; record assertion failures rather than compiler failures.
   - Disposition: `DEFER`. Direct source inspection establishes that the committed tests would reject those mutations, and no source defect follows from the evidence overstatement.

2. `SMELL` — Required-schedule coverage is strong, but not every distinct new behavior has a direct negative regression. The suite at [fs_custody.rs:5048](/Users/wesleyjinks/code/.a2a-implement/impl-66546-s8d4i725/crates/bridge-core/src/fs_custody.rs:5048) does not directly exercise anchor replacement, same-custody mutex queuing, the equal root/lock-name constructor refusal, or the non-Unix refusal surface at [fs_custody.rs:1009](/Users/wesleyjinks/code/.a2a-implement/impl-66546-s8d4i725/crates/bridge-core/src/fs_custody.rs:1009).

   - Trigger: a future A3/A4 refactor independently drops the anchor checks or mutex while retaining the tested parent/root/flock behavior.
   - Likelihood: `rare` now because there are no production callers; `plausible` after A3/A4 integration.
   - Exposure/impact: future journal runs could accept a stale anchor or return spurious contention instead of serializing same-cell operations.
   - Fix/cost: small, roughly 40–80 test lines. Add deterministic anchor-replacement-before/after-flock tests, a channel-ordered two-thread mutex test, constructor-collision coverage, and a `cfg(not(unix))` refusal test.
   - Red regressions: removing the anchor rewalk must make the anchor test return authority; removing the mutex must make the second same-cell operation return `WouldBlock` before the first releases.
   - Disposition: `DEFER`. Current production code contains the required checks and production remains unarmed.

## Evidence assessment

The checked-out identity is exactly parent `5cbeea1ed882afe448d3825984af9a3ed74bcb58` and head `3890fa6c295abcf92055940816c162c781d824bf`; the committed worktree is clean.

The implementation satisfies the A2 source contract:

- `open` performs no creation and verifies mandatory device/inode/birthtime for the anchor, parent, root, and sibling regular lock.
- `begin_operation` executes mutex → descriptor-relative lock open → exact lock identity → nonblocking object flock → retained and freshly re-walked route proof. A failed proof drops the constructed guard and unlocks.
- The fresh walk starts at the trusted anchor’s canonical path and uses no-follow child opens. Parent/root substitutions before acquisition, during contention, or immediately after flock are refused.
- A lock substituted before open fails identity/type verification. A name substitution after open cannot change the verified descriptor being flocked; peers using the same trusted binding refuse the replacement object.
- The operation guard’s fields are private, has no accessor or `Debug` projection, and releases through the existing unlock helper.
- No rename, link, copy, exchange, creation, or weaker identity fallback exists in the new production hunk.

The required schedule is source-discriminating: deleting the post-flock proof, moving it before flock, weakening lock identity/type checks, omitting real flock, releasing early, recreating the root, or falling back after `ENOTSUP` would fail specific assertions at [fs_custody.rs:5146](/Users/wesleyjinks/code/.a2a-implement/impl-66546-s8d4i725/crates/bridge-core/src/fs_custody.rs:5146). The caveat is that no admissible behavioral mutation run was supplied.

The five disclosed concerns resolve as follows:

1. Deferring V1 method deletion to A4 is sound. The V1 methods are unchanged and have no repository callers outside colocated tests; A2 neither uses nor legitimizes them. They must still be removed before A4/production arming.
2. Contention remains distinguishable as `FsCustodyError::Io` carrying `ErrorKind::WouldBlock`. There is no production caller that can collapse it into an unsafe outcome, so no current incorrect result exists.
3. Production `begin_operation` is the private seam instantiated with real `flock_nb` and a no-op hook. All other seam references are colocated tests, so the injected path is unreachable from production.
4. The retained-descriptor checks plus fresh no-follow anchor→parent→root walk exclude the scheduled substitutions before authority returns. Object-level flock remains bound to the verified lock descriptor. This conclusion depends on route-replacement peers honoring the advisory sibling lease after authority is returned; an arbitrary writer that ignores flock is not prevented by this mechanism.
5. `flock_nb` changed only from module-private to `pub(crate)` at [liveness.rs:11](/Users/wesleyjinks/code/.a2a-implement/impl-66546-s8d4i725/crates/bridge-core/src/liveness.rs:11); its body is byte-unchanged. Its only new non-test consumer is this custody implementation.

Scope is contained. New V2 route symbols occur only in `fs_custody.rs`, its colocated tests, and the handoff. There is no persistence encoding or production caller. The only production API configuration still assigns `resource_flight_route_v3 = None`; the sole `Some(...)` site is inside a test module. The exact hunks are 212 custody production additions, 244 test additions, one liveness add/delete, and 25 handoff additions: 214 production and 483 total changed lines, within both caps.

The supplied exact-head host result—4,004 passed, 0 failed, 13 ignored—is corroboration, not rerun evidence. The in-container whole-bin failure has no reachable causal path from this additive, uncalled surface and the exact head is host-green, supporting non-attribution; an exact-parent rerun in that same container was not supplied here, so I do not treat the environment classification as independently confirmed.

Confidence: **95/100**. Behavioral mutation receipts plus anchor/mutex tests and an exact-parent same-container control would raise it. A newly discovered production caller or cross-platform refusal mismatch would lower it. Evidence that binding is loaded from the mutable root, that a production route-replacement path can ignore the bound flock, that the guard exposes its file/path, or that production V3 is armed would collapse it.

VERDICT: APPROVE
SUMMARY: A2’s trusted route and exact sibling-lock authority are source-correct and scope-contained; defer the two non-blocking regression-evidence smells before production arming.