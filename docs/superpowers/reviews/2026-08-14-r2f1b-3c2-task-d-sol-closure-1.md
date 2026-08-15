I’ll perform the single authorized review round, entirely read-only. I’ll bind the checkout to the requested SHAs, read the full diff and complete affected module, then trace exports, callers, persistence/recovery, settlement, observation, and the stated regressions. The navigation skills’ instruction files are outside this repository, so the repository-only cap requires the allowed search fallback.The checkout matches the requested clean head and three-commit range. The actual range changes only the owned module and the handoff (974 added, 25 removed); `lib.rs` has no range diff, so I’ll inspect it as an export surface rather than count it as changed scope.The first-poll fence and private failed-arm call path are structurally narrow in the implementation: the inner future is boxed but untouched until `arm_now` returns, and the privilege is passed only from that error branch. I’m now checking the harder concurrency and durable-outbox interactions, including what concurrent settlers and observers can truthfully conclude while a publisher callback is in flight or refused.## WRONG findings

1. **WRONG — BLOCKER: a second send wrapper can misuse the zero-poll privilege and publish `accepted=false` after another wrapper has sent.**

   `arm_provider_send` takes `&self` and has no request-wide one-shot guard, so multiple wrappers can coexist ([construction](/Users/wesleyjinks/code/.a2a-implement/impl-2561-z43q222o/crates/bridge-core/src/remote_request_flight.rs:1442)). Each wrapper tracks only its own `arm_attempted`. Every `arm_now` error—including `InvalidStateTransition` because another wrapper already armed the row—enters the privileged settlement branch ([error branch](/Users/wesleyjinks/code/.a2a-implement/impl-2561-z43q222o/crates/bridge-core/src/remote_request_flight.rs:1584)), which may consume `ProviderSendArmed` as `Failed,false` ([allowance](/Users/wesleyjinks/code/.a2a-implement/impl-2561-z43q222o/crates/bridge-core/src/remote_request_flight.rs:1006)).

   Constructible schedule: create two wrappers from one request; poll the first until its provider future performs/accepts the send and returns `Pending`; poll the second. Its arm fails because the row is already armed, but it invokes `failed_arm=true`, durably publishes `Failed,false`, and may retire the row. The first future can then continue despite the false terminal.

   Trigger: retry, cancellation, or duplicate-dispatch code wraps the same shared request twice. **Likelihood: plausible once integrated** because the public API explicitly permits shared borrowing; current production exposure is zero because no caller is armed. Impact is critical custody inversion: an accepted remote effect is reported as definitely unaccepted.

   Bounded fix: add a request-wide, irreversible send-wrapper claim/linear token. Only its winner may perform arming and use failed-arm settlement; later wrappers must destroy their own future unpolled and refuse without changing the row. Small-to-medium, module-local change. Red regression: poll wrapper A through an accepted/Pending inner future, then poll wrapper B; B must refuse without publishing, and recovery must remain `Unknown,true`.

2. **WRONG — BLOCKER: a racing settlement can return `Ok` while publication is still pending and ultimately fails.**

   After the durable winner is placed in the watch channel, each settler calls `drive_publication` ([settlement path](/Users/wesleyjinks/code/.a2a-implement/impl-2561-z43q222o/crates/bridge-core/src/remote_request_flight.rs:1495)). If another caller owns `publication_claimed`, the method immediately returns `Ok(())` ([claim path](/Users/wesleyjinks/code/.a2a-implement/impl-2561-z43q222o/crates/bridge-core/src/remote_request_flight.rs:1529)).

   Constructible schedule: settler A persists the winner and blocks inside the publisher; settler B observes that winner, loses the claim CAS, and returns `Ok(outcome)`. The publisher then refuses, so A returns `PublicationRefused`, the row remains pending, and admission remains blocked. B has therefore observed successful settlement before the outbox attempt’s result was known. A related stale precheck allows a caller that read `publication_complete=false` to win the claim after A completed, republish, and then fail because the row was already retired.

   Trigger: two completion/cancellation sources settle concurrently while the sink is slow or refusing. **Likelihood: plausible**; the shipped test deliberately supports concurrent settlers, though current production has no caller. Exposed future callers can proceed on false success while durable debt remains. Severity is high operationally.

   Bounded fix: replace the Boolean claim with a joinable publication-flight state so racers receive the same completed publication result, or return an explicit in-flight disposition rather than `Ok`. Recheck completion after acquiring any claim. Medium, module-local change. Red regressions: a barrier-controlled refusing publisher must prevent every racer from returning `Ok`; a success race must not republish or return an error after retirement.

## SMELL findings

No additional SMELL was established beyond the test-evidence limitation below.

## Adjudications

- Round-1 effect-then-debt blocker: **FIXED** for a single legitimate wrapper. It attempts durable `Failed,false` without polling its inner future and conservatively leaves an armed row for `Unknown,true` recovery when that terminal CAS fails.
- Repair-round broad-CAS blocker: **PARTIAL**. Ordinary stale-flag settlement, drop, journal, and recovery paths now refuse correctly, but any second wrapper can still reach the privileged branch.
- Conservative residual: **acceptable**. Failed pre-send terminalization after an ambiguous durable arm leaves `Unknown,true`, which cannot falsely assert “not sent.”
- Zero-poll privilege scoping: **rejected**. Drop cannot race its borrowed wrapper, but non-cloneable ownership does not prevent multiple simultaneous shared-borrow wrappers.
- First-poll fence: **passes per wrapper**; the inner is not polled before a durable arm, and a failed arm destroys that wrapper’s future unpolled.
- Recovery, observation, refusal debt, peer isolation, and durable single-terminal CAS: no independent defect established. Observation holds no journal lock and uses Tokio watch/deadline machinery; durable transitions reach `Complete` only after the namespace transaction’s sync/cleanup sequence.
- Scope: confirmed. Only the owned module and handoff changed; `lib.rs` and `Cargo.lock` are unchanged, no `rustfmt::skip` exists, and exact-symbol search found no production caller or served projection.

## Evidence assessment

The six original Task D regressions have relevant current assertions, but their recorded pre-change result was missing-API compilation failure, not behavioral fail-first evidence. The two effect-then-debt repairs and stale-flag repair have stated behavioral red runs. No test covers multiple wrappers or a blocked/refusing concurrent publisher—the two failing schedules above.

Supplied green gates were not rerun under the read-only contract. Confidence: **97/100**. Deterministic barrier tests for both schedules would raise it; a documented and enforced external one-wrapper invariant or explicit “`Ok` means winner-only, not publication completion” contract would lower it. A request-global linear send permit plus a joinable publication result, with red regressions, would collapse the rejection.

VERDICT: REJECT
SUMMARY: Duplicate wrappers can publish false unaccepted custody, and racing settlers can report success before the durable outbox succeeds.