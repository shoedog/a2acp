# ADR-0041 B2c reconciliation design escalation

Status: owner-approved design pass1/1 COMPLETE, actual Sol/xhigh/read-only result READY. See [complete design and controller qualification](2026-10-05-adr0041-b2c-reconciliation-design-result.md). Owner also approved the expanded same-artifact Sonnet5.5 repair and TWO-round follow-on source-review cap; repair1 is live. Preserve the existing artifact; the result is not source approval.

Candidate `4b941d4b032cbd652accd5fb18a59e0cb12d258a`, parent `c4c60a60dfcbf1e04d0f4fe532d77233363aa82f`, whole-B2c base `dc0b45bafd3538c93fe15254bd9f975ad34f4c71`, remains isolated in `impl-70428-q52t65ks`. [Source review 4/4](../reviews/2026-10-04-adr0041-slice2b3b-b2c-source-round4.md) rejected three WRONGs and one nonblocking historical SMELL. Its classification is OPEN CLASS at the exhausted cap: successive repairs still reveal combinations in the same reconciliation family. The existing artifact is retained; no restart or fifth source round is admitted.

The owner-supplied convergence instruction, also in `/Users/wesleyjinks/.codex/AGENTS.md:13`, says: “if findings are open-class (each round surfaces new instances of the same kind), park and escalate to spec or design.” Implementation is parked under that rule. The authorized bounded contract/design review is complete. Owner approved the expanded repair scope and newly declared TWO-round follow-on source-review cap; implementation proceeds on the same artifact.

## Concrete population exposed by the final review

| Input or state | Incorrect result | Required observation / bounded direction |
|---|---|---|
| Routed final child adds `.git/0stray`, changes HEAD bytes without changing length, then fails or times out | Tree failure prevents every baseline-file digest; `Bytes` is omitted | Preserve primary and `Unexpected`; independently digest trusted baseline files with retained descriptors and existing length-plus-one bounds, recording `Bytes` |
| Routed child changes its route binary and swaps a retained bootstrap chain | Callback identity refusal replaces runner `BinaryDrift` | Retain dominant runner drift plus callback custody refusal; substitute the callback only for its synthetic runner `RouteRefusal` |
| Post-child mount census refuses before retained-chain checks, while a retained chain also changed | Any non-timeout gate refusal marks all gates complete; chain failure is omitted | Track completion per gate stage and run still-owed stages once; preserve mount refusal and chain identity failure |

Review the state model before editing: which stages ran, which completed, which produced findings, and which remain owed after a child may have run. A refusal is an observed finding; it is not proof that later stages ran. Specify error precedence separately from stage completion, so an independent runner error cannot be erased by a callback translation.

Enumerate the finite reachable combinations from actual exit sites and existing seams, rather than treating each of the three rows as an isolated patch. Cover pre-spawn refusal, post-child runner/status/parser failures, callback custody refusal, known timeout, and expiry first discovered during settlement. For each reachable state, record completed/owed mount, tree census/comparison, baseline digest, retained-chain and final-root stages. Distinguish safe independent observations from stages that require an earlier trusted fact; define typed refusal when an observation cannot safely finish. Avoid an unbounded Cartesian mutation matrix.

## Boundaries and reviewable deliverables

Keep the existing runner, public API, codec, dependencies, Pass A, single verified capsule, ledger, retained descriptors, absolute deadline, finite observation bounds and destination-root-last success rule. Failure-only reconciliation cannot produce success, retry indefinitely, write, or acquire a fresh deadline. Preserve earlier rejected checkpoints and the historical production-before-RED debt.

The design output should contain an explicit stage/completion table, primary-error composition rules, bounded treatment of unreadable or changed baseline files, and a tests-first repair plan on the same artifact. Require discriminating combined regressions for all three rows and controls for pre-spawn refusal and an unchanged successful child. Declare repair and review caps before dispatch. Implementation must use actual `claude-sonnet-5-5`; independent source review remains Sol.

Already completed exact-candidate gates remain valid evidence: fmt, workspace clippy, locked build, hygiene, all-target tests **5,192/0/13**, default tests/doctests **5,203/0/13**, focused **330/0/0**, eight targeted variants and four controls. They do not cover the three combined failures. Do not replay unchanged-source gates merely to prepare this design brief. Native macOS/ext4, aggregate integration, combined whole-2B3b review and publication remain pending behind B2c approval.

The owner's audit policy applies to the follow-on plan: expensive deep mutation/controller audits stay out of routine CI/CD and are skipped for slices that leave the affected area unchanged. Ordinary required tests/review remain. Scope any new audit to the changed reconciliation behavior and evidence; do not replay the historical 132/190/16 matrices.
