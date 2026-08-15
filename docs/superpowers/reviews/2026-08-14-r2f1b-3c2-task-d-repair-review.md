[implement] review Reject:
I’m beginning the bounded, read-only closure review. I’ll read the supplied review slice first, then verify the exact commit diff and trace the changed production path through settlement, recovery, and observation; I will not run tests or make changes. The repository-only cap prevents loading the external Prism skill file, so I’ll use the checked-in slice and any already-configured navigation tools directly if available.The review slice is empty (`LeftFlow`, no slices or findings), and no Prism/LSP navigation tools are configured in this session. I’m therefore treating it only as a map with no precomputed leads and proceeding with read-only Git/search verification.Identity and scope are exact: `HEAD` is `a072aacb`, the worktree is clean, and only the two authorized files changed. Raw churn is 199 additions plus 1 deletion (200 total), with the source file at 154 additions/1 deletion; I’m now separating production from `#[cfg(test)]` and checking the wrapper’s error path and the recovery/publication projection.The intended repair path is present: the failed first-poll arm drops the inner future, attempts `Failed/false`, persists through `terminal_winner`, and recovery republishes that durable winner. I found a separate state-guard concern in the widened CAS and am checking whether it is constructibly reachable through the public owned-request API, including concurrent settlement during the arm/atomic handoff.## WRONG findings

1. **WRONG — BLOCKER: ordinary settlement can publish `accepted=false` after a successful arm and subsequent provider poll.**

   Constructible sequence: thread A polls `ArmedProviderSendV1`; the durable transition to `ProviderSendArmed` completes at [remote_request_flight.rs:1449](/Users/wesleyjinks/code/.a2a-implement/impl-2561-z43q222o/crates/bridge-core/src/remote_request_flight.rs:1449). Concurrent thread B calls `settle()` and reads the still-false atomic at [remote_request_flight.rs:1471](/Users/wesleyjinks/code/.a2a-implement/impl-2561-z43q222o/crates/bridge-core/src/remote_request_flight.rs:1471). Even if A then stores `true`, B retains its stale `false`; the new allowance at [remote_request_flight.rs:1014](/Users/wesleyjinks/code/.a2a-implement/impl-2561-z43q222o/crates/bridge-core/src/remote_request_flight.rs:1014) lets B persist and publish `accepted=false`. A can then poll the provider future, so the durable outbox and observer report “not accepted” although the provider may have accepted it. The prior code rejected this state mismatch.

   Trigger: a successful first-poll arm racing cancellation or another terminal settler on a multithreaded executor. The type is demonstrably shareable across threads—the existing racing-settlement test does so—and the module is publicly exported. Likelihood is **rare** because the atomic handoff window is narrow; no current repository production caller exists and Tasks E–G/V3 remain unarmed. Exposed future V3 or downstream users could incorrectly treat the request as retry-safe, risking duplicate provider effects; impact is high.

   Bounded fix: preserve the old rule that ordinary `accepted=false` settlement cannot consume `ProviderSendArmed`. Add a private failed-arm settlement capability/mode used only by the wrapper’s zero-poll error branch, allowing the armed row solely there. This is localized to this file, roughly 10–25 production lines plus focused tests. Add a red regression that pauses after durable successful arm but before the atomic update, races `settle()`, then proves no `false` terminal can win and any provider-polled outcome remains `accepted=true`; also retain a lower-level test that ordinary `settle(..., false)` rejects an armed row.

   **BLOCKER:** the repair’s widening removes the state-machine guard outside the proven failed-arm path, contradicting the required “no behavior changes otherwise.” The bounded repair cost is small relative to the custody/retry-safety risk.

## SMELL findings

None established independently of the blocker.

## Evidence assessment

The exact clean `a072aacb` checkout changes only the two authorized files. Churn is within bounds: 7 production additions, 147 test additions/1 deletion, and 45 documentation additions, totaling 200 changed lines. The intended effect-then-debt repair and terminal-failure fallback are otherwise causally present, with behaviorally fail-first focused tests and the documented `Unknown,true` residual. The supplied verifier reports fmt, clippy, build, and tests passing; these were not rerun under the read-only contract. The supplied Prism slice was empty.

VERDICT: REJECT
SUMMARY: The targeted recovery repair works, but its global armed-row allowance permits a concurrent successful send to be durably misreported as unaccepted.
implement: committed a072aacbca94c2b9d01a38d71a1c9fa29583d1a8 "fix(r2f1b): settle unarmed failures before recovery can overstate them" on implement/impl-2561-z43q222o
clone: /Users/wesleyjinks/code/.a2a-implement/impl-2561-z43q222o
After an Approved result, use the guarded operator-authored hand-off (add --config when non-default):
  a2a-bridge merge impl-2561-z43q222o --onto <target>
For an inspected parallel sibling whose target advanced from the shared base:
  a2a-bridge merge impl-2561-z43q222o --onto <target> --integrate-current

verify: PASS  (fmt reached exit=0 ✓ · clippy reached exit=0 ✓ · build reached exit=0 ✓ · test reached exit=0 ✓)
review: REJECT  (The targeted recovery repair works, but its global armed-row allowance permits a concurrent successful send to be durably misreported as unaccepted.)
loop: 1 attempt(s) — bound reached
dde5ba01fb374e3f0b53dc6e18ed8be53149ff5a0dac4c3794cc8ebff782b30c
