Prior-round adjudication:

- PARTIAL — Recovery ownership: the operator was right that removing the currently hardcoded-false input had no demonstrated production failure. The revised repair is still incomplete because it requires a reachable refusal while forbidding construction and defining no ownership input.
- PARTIAL — Preparation journal wording: the main description now includes `schema_version`, but the proposed commit message again omits it.
- The other ten round-1 findings were not individually reproduced, so they cannot be defensibly adjudicated one by one from these inputs.

Disagreement resolution: Rigor is right that the guard-refusal vocabulary is a BLOCKER, not Soundness’s MAJOR, because two implementations can return different typed results for the same out-of-root record while both claiming compliance with AC3.

1. BLOCKER — WRONG — Section: “Signature migration” / AC6. Issue: recovery ownership must remain reachable through V3 assessment, but `LocallyOwned` and `OwnershipCannotProve` must be constructed nowhere, and no ownership input or assessment signature is specified. An implementer must either make the required path unreachable, construct a forbidden variant, or retain an undocumented wrapper. Suggested resolution: define the ownership-observation input, exact V3 assessment signature, mappings, and whether “constructed nowhere” means production-only; alternatively defer both variants and reachability to slice 2.

2. BLOCKER — WRONG — Section: “Population projection” / AC1. Issue: the table is not exhaustive over its declared `(state, claim presence)` key, omitting required-claim states with absent claims and forbidden-claim states with present claims. It also permits a refusal wildcard, so adding a future state could compile instead of forcing review. Suggested resolution: structurally accept only decoder-validated records, or enumerate every pair and prohibit all state wildcards.

3. BLOCKER — WRONG — Section: “Degraded-claim handling” / AC10. Issue: field-level behavior is delegated to the implementer. Current construction treats source identity, common-directory identity, worktree identity, and the unused `root` field differently, so one implementation could probe a degraded worktree while another returns `CannotConstructSubject`. Suggested resolution: provide an authoritative source/root/worktree/common-directory matrix with exact constructibility and refusal outcomes.

4. BLOCKER — WRONG — Section: “Guards that must survive” / AC3. Issue: no proposed refusal variant or precedence exists for outside-root and record-file mismatch failures. The same outside-root record could return `IneligiblePopulation` or `CannotConstructSubject`, breaking typed-result consumers and exact tests. Suggested resolution: add explicit `OutsideSweepRoot` and `RecordFileMismatch` variants, or prescribe exact existing mappings and guard precedence.

5. BLOCKER — WRONG — Section: “Who runs which gate” / red-first battery. Issue: a final test asserting the new `IneligiblePopulation` type cannot compile on unmodified `9aedf175`; treating that compile failure as the required pre-change failure would provide no behavioral evidence. Suggested resolution: split base-compatible probe-count tests from post-change vocabulary assertions, or define a behavior-neutral test scaffolding patch; explicitly make compile/setup failures inadmissible.

6. BLOCKER — WRONG — Section: red-first and guard fixtures. Issue: zero probe calls can false-green when `from_claim` rejects synthetic source/common-directory authority first. Likewise, a naive outside-root fixture may also fail the sibling guard, so deleting only the intended guard remains undetected. Suggested resolution: pre-prove candidate constructibility with real bound Git authority and a one-probe control; isolate the outside-root guard with an in-root symlink resolving outside and make each guard test assert the other guard passes.

7. BLOCKER — WRONG — Section: “Regression and exit coverage” / AC7. Issue: `sweep_orphans_with_exact_absence` returns `()` and only logs decisions, yet the four-arm test must assert exact typed assessments through that production path. A test could otherwise verify programmed probe inputs and unchanged bytes while a wrong adapter mapping passes. Suggested resolution: specify a private typed collector/reporting seam shared by production traversal and tests.

8. BLOCKER — WRONG — Section: “Effect freedom” / AC7. Issue: final snapshots and a local search for mutation calls do not exclude a transitive helper that mutates and restores a record or directory. Suggested resolution: require an operation-recording seam across the full traversal or an explicitly transitive call-graph audit excluding every mutation-capable reachable callee.

9. MAJOR — WRONG — Section: “The defect this slice closes.” Issue: “every claim-bearing V3 record reaches the proof” is false because degraded or stale source/common-directory authority can fail during candidate construction before probing. Suggested resolution: say “every constructible claim-bearing V3 record attempts the exact-absence proof.”

10. MAJOR — SMELL — Section: advisory handoff. Issue: “T3b must re-run the complete proof” does not explicitly require rereading the guard-bound record and rerunning admission, ownership, and exact absence under T3b’s lock. A future implementation could reuse the returned subject after the record changes. Suggested resolution: enumerate the complete locked reproof sequence and state that no T3a observation is reusable authority.

11. MAJOR — SMELL — Section: AC12 line-cap evidence. Issue: `git diff --numstat 9aedf175..HEAD` ignores staged, unstaged, and untracked bytes, so it can green while the actual slice exceeds 400 lines. Suggested resolution: require a clean, fully committed worktree before this command or explicitly count index, worktree, and untracked handoff changes.

12. MINOR — WRONG — Section: “Commit Message.” Issue: it says the preparation journal carries only flight ID and state, omitting the verified schema version. Suggested resolution: include `schema_version`; the no-authority conclusion remains unchanged.

13. MINOR — SMELL — Section: sizing criteria. Issue: the dispatch spec sets a 400-line cap while the enclosing review criterion still says 250, creating contradictory approval gates. Suggested resolution: update the review criterion to 400; that limit is tight but plausible with table-driven fixtures.

VERDICT: Not ready to plan; resolve the eight BLOCKER contract and evidence defects first.