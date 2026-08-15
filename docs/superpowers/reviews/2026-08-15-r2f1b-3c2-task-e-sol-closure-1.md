node_terminal{node="review",terminal_json={"schema_version":1,"primary":"completed","cleanup":{"disposition":"complete","duration_ms":0},"cause":null,"prompt_may_have_been_accepted":false,"degraded_ancestry":false,"policy_trigger_id":null}}
I’m beginning the single capped closure-review pass. I’ll freeze the checkout identity, read both requested artifacts completely, inspect the exact three-commit diff and production caller/persistence/projection paths, then adjudicate all four inherited WRONGs and the deferred test SMELL. I will not edit, build, test, invoke providers, or read outside this repository; that boundary also prevents loading the external Prism/LSP skill instructions, so I’ll use read-only Git/search navigation.Identity matches the supplied head and branch, the worktree is clean, and the exact range changes only the two declared files. The closure is larger than the production delta suggests (`backend.rs` is 3,786 lines and the handoff 1,108), so I’m reading both in bounded numbered chunks before relying on the focused diff.Both requested artifacts are now read through EOF. The Task E production path is concentrated in the custodian, request-scope drop transfer, session cleanup, and the four trait cleanup surfaces; next I’m comparing each commit’s actual patch and tracing those surfaces into bridge-core wrappers and terminal projections.The current-state rewrite fixes the reported `settle_drop` race, but the same cell still has a separate terminal writer: ordinary `RequestScope::settle` calls `finish()`, which unconditionally replaces `TimedOut`. I also found that both success tails synthesize `acknowledged=true`, although the retained-flight publisher is a void callback and defaults to a no-op; the newer Task D publisher is the only surface that requires an exact delivery-ID echo. I’m finishing the severity/reachability and regression assessment now.## WRONG findings

1. **WRONG — V3 cleanup fabricates a publication acknowledgement.**  
   Constructible state: configure the public V3 route with `DurableProcessFlightAttemptV3::new`, whose publisher defaults to `NoopResourceFlightResultPublisher`, and complete an API request. Both normal settlement and late-drop success publish `acknowledged=true` ([backend.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-api/src/backend.rs:821), [backend.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-api/src/backend.rs:548)). The projection consequently returns `Complete` ([backend.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-api/src/backend.rs:582)).

   The old flight’s publisher returns `()` and may be a literal no-op ([retained_resource_flight.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-core/src/retained_resource_flight.rs:1015), [process.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-core/src/process.rs:887)); it supplies no exact durable acknowledgement. By contrast, Task D requires an exact delivery-ID echo before finishing publication ([remote_request_flight.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-core/src/remote_request_flight.rs:1603)). The incorrect `Complete` can propagate through worktree composition and workflow terminal persistence.

   Trigger: any library embedder arming `ApiResourceFlightRouteV3`, especially with the default/no-op publisher. The shipped binary currently sets the route to `None`, so current CLI users are not exposed, but this is common for any such V3 route and directly violates Task E’s binding acknowledgement criterion. Impact is high: unacknowledged custody is reported reclaimable.

   Bounded fix: pass `acknowledged=false` for the old adapter, or thread the Task D exact-acknowledged outcome into the cell before permitting `Complete`. Add a public-path regression proving a successful request with the no-op publisher cleans up as `Unknown`, plus a positive exact-echo case when the Task D driver is wired. Cost: small safety change now; moderate V3 integration blast radius. **BLOCKER** because approval would bless the exact false-success state Task E was meant to exclude.

   Confidence: **96/100**. A behavioral no-op-publisher regression would raise confidence; a binding contract proving that callback return—even from `Noop`—is the required durable acknowledgement would lower it; an actual exact delivery echo feeding this boolean would collapse the finding.

2. **WRONG — ordinary request settlement can still overwrite `TimedOut`.**  
   The completion makes `TimedOut` absorbing only inside `settle_drop`. Normal `RequestScope::settle` instead calls `finish()` ([backend.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-api/src/backend.rs:805)), whose identity match unconditionally changes the state to `Terminal` ([backend.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-api/src/backend.rs:426)).

   Constructible schedule: a V3 API response reaches successful settlement; its synchronous result publisher blocks after the durable append; concurrent checked cleanup enters `DropOwned`, removes the session, and expires to `TimedOut` ([backend.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-api/src/backend.rs:644)); then the publisher returns and `finish(... Complete, true)` overwrites the timeout. The cell becomes reclaimable and can be removed or return `Complete` on a later cleanup, erasing timeout debt.

   Trigger: an armed V3 route, multi-thread runtime, concurrent forget/release, and slow publisher or storage near the deadline. Likelihood is **rare** at this head because production routing is unarmed, but it is reachable through the public route and becomes directly exposed on activation. Impact is high: an expired, previously unknown cleanup is later upgraded to reclaimable.

   Bounded fix: centralize exact terminal publication so `finish()` also inspects current state under its lock; when current state is `TimedOut`, retain that state and record terminal evidence without upgrading. Add a public `prompt` → concurrent `forget_session_checked` regression with a barrier publisher, covering late `Complete` and refusal/non-complete edges. Cost: low-to-moderate and confined to `backend.rs`. **BLOCKER** because `TimedOut` is not actually absorbing across all production settlement paths.

   Confidence: **98/100**. A deterministic public-path barrier test would raise confidence; proof that successful `RequestScope::settle` cannot overlap checked cleanup would lower it; serialization or a current-state check inside `finish()` would collapse it.

## SMELL findings

1. **SMELL — the direct-cell tests do not bind the production paths they claim.**  
   The crossing regressions invoke `settle_drop` directly and even inject `Complete`, although production `RequestScope::drop` proposes only `Failed`, `Partial`, or `Unknown` ([backend.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-api/src/backend.rs:833), [backend.rs](/Users/wesleyjinks/code/.a2a-implement/impl-59023-lbpwrndo/crates/bridge-api/src/backend.rs:3618)). They therefore missed the actual successful `RequestScope::settle` writer. The recreation extension also keeps the old cell unbound, so it does not behaviorally exercise same-spelling bound Legacy authority.

   Trigger: future changes that preserve cell-unit behavior while breaking admission, normal settlement, wrappers, or recreation. Likelihood is **plausible**; all API cleanup users and downstream terminal projections are exposed. Impact can be high, as WRONG 2 demonstrates.

   Proposed repair: add public-path tests for normal-settlement deadline crossing, bind/publication-window cleanup, and a genuinely bound forgotten/recreated Legacy session. Cost is modest and test-only. **DEFER** as a separate ledger item after the two blocker regressions are mandatory; source structure still supports the stale-authority claim.

   Confidence: **99/100**. Committed public-path regressions would lower or collapse this assessment.

## Adjudication and evidence

- Base Clippy WRONG: **FIXED** — `PreparedRequest::Ready` is boxed and admission uses `inspect_err`.

- Base diagnostic-custody WRONG: **FIXED** — observation clones the diagnostic and clears only the exact still-current value after a confirmed observer write; expiry, refusal, and timeout retain it.

- Base timeout/drop-custody WRONG: **FIXED for the reported drop path** — late drop transfer is accepted, failure retains the flight, and no after-deadline retry is admitted.

- Repair-round `settle_drop` crossing WRONG: **FIXED** — the tail branches on current state under one lock. WRONG 2 is a distinct surviving terminal writer.

- Operator concerns: the `#[cfg(test)]` gate is absent from production and cannot perturb production behavior; the inlined `settle_drop` success/refusal behavior has parity and unconditional notification only causes harmless wakeups; the acknowledgement claim is unsound as described in WRONG 1; the stale `timed_out` snapshot gates only retry safely because `TimedOut` is assigned only after deadline expiry and retry separately requires `now < deadline`.

- The remaining table behavior is source-supported: overlapping Legacy work stays `Unknown`; bind/publication-window cleanup is protective; refusal and timeout cells outlive the session slot; accepted drop diagnostics retain custody; completed non-overlapping cells are reclaimable; turn authority and exact cell references fence recreated sessions; deadline observation uses Tokio waits and the RAII count returns to zero.

- The four cleanup surfaces converge on `cleanup_session_checked`; protective values survive worktree combination, workflow aggregation, and terminal persistence. That same chain makes either false `Complete` finding consequential.

- Exact identity remained clean at `1f3c3a82`; the range is exactly three commits and changes only `backend.rs` plus the handoff. `Cargo.lock` is unchanged, no `rustfmt::skip` was added, and production still assigns the V3 route `None`. Task A–D source files are untouched, and `backend.rs` still uses the old adapter rather than Task D’s driver; I found no source-level regression in those families.

- I did not build or run tests under the read-only contract. The supplied 4,084/0 suite and other green gates are corroboration only. Overall confidence: **97/100**. Reproducing both proposed public-path red tests would raise it; contrary binding specifications or serialization proof would lower it; exact acknowledged publication plus absorbing normal settlement would collapse the rejection basis.

VERDICT: REJECT
SUMMARY: The four reported defects are fixed in their named paths, but V3 still fabricates publication acknowledgement and ordinary successful settlement can erase TimedOut debt.