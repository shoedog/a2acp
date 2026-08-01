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
second Sol review of that production repair tree was launched. The review verdict therefore remains a rejection
of the frozen pre-repair tree; it is not relabeled as an approval of later bytes. A distinct post-merge review
later inspected only the CI fixture repair and is recorded below.

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
- Nine compatibility-schedule tests failed during fixture-foundation setup on both the repaired candidate and
  immediate base `ce38a4e`. That controls only the six-finding repair delta. Exact pre-R2f0b main `1a8cfc0`
  passed the same GitHub workflow, while post-merge run `30704164902` failed the nine tests. The earlier defer
  was therefore superseded: this was a cumulative R2f0b `WRONG`, repaired at `2744cb1` without weakening
  production admission.
- One controller concurrency observation lacked a same-harness base control. It does not establish a
  regression and remains deferred rather than silently attributed to R2f0b. A later intermittent local macOS
  operation-lock test failure remains outside the fixture diff; its exact and full-crate controls passed alone.

## Post-merge fixture review and folded finding

One distinct bridge-mediated Sol/xhigh/read-only review inspected the exact uncommitted fixture diff against
`666abb8a24f61b685219ac725f5e533b31f818a4`:

- execution: `exec-e8a0ef8a7efc75f9a2847e9c2e086d5a`;
- attempt: `attempt-0e4f387ac85602dcaa4563c3b35b8219`;
- result: 5,553 bytes, SHA-256
  `d7e4b8e2715dbc2e3659352426bffe3a5c5ad5fbbc6c24b6d16ef4735e47452c`;
- verdict: `APPROVE`;
- findings: one rare test-only `WRONG / DEFER`, no `SMELL`.

The finding constructed an owner host where `/Users/wesleyjinks/code` is a symlink. Canonicalizing the
generated child before the production resolver's lexical check made the fixture falsely reject an otherwise
valid rooted directory. Although nonblocking and inherited from the earlier fixture, the owner directed valid
findings to be folded. A Unix regression failed **0 / 1** on the reviewed construction, then passed **1 / 0**
after returning the retained temporary directory's lexical path and leaving canonical containment to the
production resolver. Final repair commit `2744cb13db336b8fa99db9c48e638b99c161fd82` passed the complete schedule
slice **302 / 0**, full workspace **3,089 / 0 / 12 ignored**, static gates, and green replacement CI run
`30706571173`.

## Acceptance boundary

The operator authorized the initial review, every closed repair, fail-first controls, full deterministic suite,
landing, and the distinct post-merge Sol fixture review. No compatibility canary, production-server replacement,
release, or operator deployment is part of this acceptance.
