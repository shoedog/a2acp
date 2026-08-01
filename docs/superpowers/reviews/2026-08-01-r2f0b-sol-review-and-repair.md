# R2f0b Sol/xhigh correctness review and repair disposition

Date: 2026-08-01

## Frozen scope and verdict

One operator-authorized Sol/xhigh review inspected the complete cumulative R2f0b production path in the
quarantined implementation clone. The read-only review was frozen to custody snapshot
`681ff6da6521a743329c3ef0ca2007907ce97705`, tree
`be207def30c3debaa29e370379ea57e1d692bfd9`, on immediate base
`ce38a4ef53bde58aa67f60b3558be532e35c0a32`.

The reviewer returned `REJECT`: six concrete `WRONG` findings were blockers and four `SMELL` findings
needed disposition. Per the declared one-review cap, the repair stayed on the existing artifact and no
second Sol review was launched. The review verdict therefore remains a rejection of the frozen pre-repair
tree; it is not relabeled as an approval of later bytes.

## WRONG findings and closed repairs

| Finding | Constructible production condition and wrong result | Exposure and risk | Bounded repair and evidence |
|---|---|---|---|
| Plain unary reached count | A served direct unary prompt crosses the provider dispatch boundary, but its durable terminal row records `reached=0` and can project the turn as not applicable. | Common on the direct-unary route; high confidence and blocking because accepted work is omitted. | Register the one provider leg at the durable dispatch boundary. Exact-base red and repaired-tree green: `r2f0b_plain_unary_counts_only_a_reached_provider_turn`. |
| Workflow preflight omitted | A configured workflow preflight sends one or more provider prompts before the real node, but those prompts are absent from attempt evidence and can leave an incorrect exact-one projection. | Plausible whenever model preflight is enabled; blocking because real billable/provider work is invisible. | Bind preflight turns to the workflow attempt telemetry. Exact-base red and repaired-tree green include `dispatcher_preflight_runs_before_warm_checkout_when_enabled`. |
| Workflow leg registered before poll | Cancellation wins after setup but before the provider future receives its first poll; the old ordering nevertheless records a reached leg and can derive the wrong terminal state. | Rare but reachable at a cancellation boundary; blocking because it invents provider work. | Move leg registration behind the poll/dispatch observer. Exact-base red and repaired-tree green include `prompt_dispatch_barrier_completes_before_provider_poll` and `cancellation_before_prompt_poll_does_not_claim_acceptance`. |
| Fan-out response loss omitted | A delegated peer accepts a request and the response path disconnects before `delegate()` returns; the old caller omits that dispatched peer from terminal counts. | Rare distributed ambiguity, but realistic under transport loss and blocking because accepted work disappears. | Observe peer dispatch before waiting for its response. Exact-base red and repaired-tree green: `r2f0b_unary_fanout_counts_peer_dispatched_before_response_loss`. |
| Collector cap was not sticky | A configured workflow/retry population exceeds 1,024 legs; retained evidence and projected counts disagree because truncation is not recorded as information loss. | Uncommon at current sizes but constructible from accepted configuration; blocking because bounded evidence can look complete. | Retain the exact cap and set sticky overflow/incompleteness for every later leg. Exact-base red and repaired-tree green: `r2f0b_workflow_collector_retains_exact_cap_and_marks_later_loss`. |
| Implement review custody missing | The ordinary implement review route uses no rich sink and the warm wrapper supplies unsupported terminal evidence, so review activity and v1 evidence are dropped. | Common for implement review; blocking because an active or accepted review can appear silent or absent. | Bind review to attempt-owned activity and terminal-evidence telemetry through warm repair turns. Exact-base red and repaired-tree green: `r2f0b_implement_review_uses_attempt_owned_activity_and_terminal_evidence`. |

All six closed repairs are present in custody snapshot
`97f51144ad05ffb9ab1d2a1eca2c601ef1582548`, exact tree
`8bc7e3c43417ad650e2426756b4aafc5aca2582a`. The repaired tree also makes one Sol/xhigh, hard-read-only,
correctness-first review the default and requires likelihood, real-world trigger, impact, bounded fix, and
blocker/defer triage. The former dual-review workflows remain explicit opt-ins.

## SMELL dispositions

- The missing fail-first control was an acceptance gap. It was closed by running the six behavioral
  controls against exact base `ce38a4e` and the repaired tree; base was red and the repaired tree green.
- A provider/tool transition counter can theoretically exceed `u64::MAX`. No realistic execution reaches
  that population; defer until the counter boundary is otherwise changed.
- Nine compatibility-schedule tests fail during fixture-foundation setup. The same nine names fail on the
  exact pre-repair base in the same environment before the intended behavior is reached; defer as an
  independently owned fixture repair and do not count those exits as behavioral evidence.
- One controller concurrency observation lacked a same-harness base control. It does not establish a
  regression and remains deferred rather than silently attributed to R2f0b.

## Acceptance boundary

The operator authorized the one review, every closed repair, fail-first controls, the full deterministic
suite, and landing. No live provider turn, compatibility canary, production-server replacement, release,
or operator deployment is part of this acceptance.
