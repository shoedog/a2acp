[implement] review Reject:
I’ll conduct a hard read-only review of the exact base-to-candidate diff. I’m starting with the supplied review slice, then I’ll trace the owned-request state machine, journal/outbox persistence, recovery, observation, and tests; I will not run builds or tests. The repo-only cap prevents loading external navigation skill files, so I’ll use only already-configured navigation if directly callable and otherwise bounded Git/search.The review slice is a five-line empty map (`slices: []`, `findings: []`), so it contributes no defect leads. I’m falling back to the exact committed diff and caller/persistence tracing within the repository.The candidate is within the stated total churn cap (735 changed lines) and changes only the owned module plus handoff. I’m now checking the durable state transitions against Task C recovery and looking specifically at failure paths that the six new tests do not exercise, especially non-`Refused` Task A outcomes and concurrent settlement/publication.## WRONG findings

1. **WRONG — BLOCKER: a failed arming transaction can recover as `Unknown, accepted=true` even though the provider future was never polled.**

   Constructible sequence:

   - The request is `DispatchAuthorized`.
   - During the wrapper’s first poll, `NamespaceTransactionV2::replace` publishes the `ProviderSendArmed` successor, then a sync/verification/cleanup failure returns `Retained` or protective debt rather than exact `Complete`. The transaction supports precisely this post-publication failure shape at [namespace_transaction.rs](/Users/wesleyjinks/code/.a2a-implement/impl-869-nmcagyux/crates/bridge-core/src/namespace_transaction.rs:546).
   - `transition_state` marks the journal as requiring reopen ([remote_request_flight.rs](/Users/wesleyjinks/code/.a2a-implement/impl-869-nmcagyux/crates/bridge-core/src/remote_request_flight.rs:924)). The wrapper destroys the inner future without polling it, but its attempted `Failed,false` settlement is rejected by `requires_reopen` and silently discarded ([remote_request_flight.rs](/Users/wesleyjinks/code/.a2a-implement/impl-869-nmcagyux/crates/bridge-core/src/remote_request_flight.rs:1539)).
   - Reopen completes Task A recovery, sees the durable armed row, and publishes `Unknown,true` ([remote_request_flight.rs](/Users/wesleyjinks/code/.a2a-implement/impl-869-nmcagyux/crates/bridge-core/src/remote_request_flight.rs:867)).

   The incorrect observable result is therefore `prompt_may_have_been_accepted=true` with a provider-future poll count of zero, violating both “only exact `Complete` advancing” and failed-arm `Failed,false`.

   Trigger likelihood is **rare**, requiring a filesystem/namespace failure after successor publication but before Task A returns `Complete`. It is nevertheless a first-class custody failure mode, not theoretical. Production V3 is currently unarmed, so no current in-repo caller is exposed; once Task E/F wires this driver, every provider run encountering that cut can receive a false acceptance outcome, potentially suppressing a safe retry.

   The existing regression is nondiscriminating: its special hook refuses before running the replacement adapter ([remote_request_flight.rs](/Users/wesleyjinks/code/.a2a-implement/impl-869-nmcagyux/crates/bridge-core/src/remote_request_flight.rs:917)) and tests only the no-effect `Refused` case.

   **Bounded fix:** retain module-local provenance from `NamespaceTransactionV2::recover`; when recovery completes an interrupted replacement targeting an armed request row, terminalize it as `Failed,false`. Preserve `Unknown,true` for an armed row with no interrupted-arming provenance, covering the exact-`Complete` crash-before-poll window. Expected cost is medium and localized to this module. The red regression should run the real replacement through a post-publication retained/debt cut, assert zero inner polls, reopen, and assert `Failed,false`; retain the exact-Complete post-arm `Unknown,true` control.

## SMELL findings

None established beyond the blocker’s missing post-publication failure regression.

## Evidence assessment

The exact candidate head and two-file diff were inspected completely. Churn is 735 lines total and 428 production lines, within both caps. No production caller or route references the new driver, `lib.rs` is unchanged, and the supplied prism slice was empty. The six new tests cover happy-path arming, no-effect refusal, prefix recovery, ordinary settlement racing, observation timeout, and publication refusal, but not the blocker’s real post-publication/non-`Complete` arming state. The supplied verifier reports PASS; no build or test was rerun under the read-only contract.

VERDICT: REJECT
SUMMARY: Failed arming can recover `Unknown,true` after zero provider-future polls when Task A publishes the armed row but returns a non-Complete debt outcome.
implement: committed bd29eddf4759f210306d27e3e25c2dd782f86cc2 "feat(r2f1b): add owned request driver with first-poll arming" on implement/impl-869-nmcagyux
clone: /Users/wesleyjinks/code/.a2a-implement/impl-869-nmcagyux
After an Approved result, use the guarded operator-authored hand-off (add --config when non-default):
  a2a-bridge merge impl-869-nmcagyux --onto <target>
For an inspected parallel sibling whose target advanced from the shared base:
  a2a-bridge merge impl-869-nmcagyux --onto <target> --integrate-current

verify: PASS  (fmt reached exit=0 ✓ · clippy reached exit=0 ✓ · build reached exit=0 ✓ · test reached exit=0 ✓)
review: REJECT  (Failed arming can recover `Unknown,true` after zero provider-future polls when Task A publishes the armed row but returns a non-Complete debt outcome.)
loop: 1 attempt(s) — bound reached
f7069389404330a5ec60799c0725bbfabdfea835138cf4e1d2f724ad943d9f47
