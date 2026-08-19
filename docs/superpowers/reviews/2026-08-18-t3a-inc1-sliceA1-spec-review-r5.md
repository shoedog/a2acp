PRIOR-ROUND ADJUDICATION

- FIXED — Removed-accessor demand: the spec explicitly prohibits a scalar effective-decision accessor.
- FIXED — Fifteen-type count and public re-export contract.
- FIXED — Authority-lifetime language and mandatory T3b re-decision.
- FIXED — Readiness-seam testability while production readiness remains false.
- FIXED — Scanner module visibility and parent-module integration.
- FIXED — AC 17 now preserves the wrapper’s guard-root canonicalization, warning, early return, and raw action-scan root.
- FIXED — AC 19 now includes the path-identity comparator and pin-open observation trees.
- FIXED — Remaining supplied round-3 corrections; neither current lens identifies a regression.

BLOCKER

1. WRONG — A2 mutation audit / AC 20 / falsification license

   Issue: The universal claim that every non-byte-identical porcelain registration path reaches `compare_path_identities` is false. With a non-byte-identical path field containing invalid UTF-8, `registration_absent_from_porcelain` returns `ConfigInvalid` before invoking the comparator. An implementer must therefore either assert a nonexistent call edge or stop under the falsification rule.

   Suggested resolution: Restrict the comparator inventory to non-byte-identical, valid-UTF-8 registration fields. Add the UTF-8 decode-refusal branch to the normative inventory and require evidence that it preserves the raw `Refused` projection.

MAJOR

2. SMELL — A2 compatibility/action scan separation

   Issue: The single symlinked-root test combines a stable success case proving canonical exact-scan versus raw action-scan spelling with a counterfactual guard-canonicalization failure. Without a specified between-phase seam, the same fixture cannot deterministically prove both the successful raw scan and the wrapper’s warning/early-return behavior.

   Suggested resolution: Split this into a stable alias-path test and a deterministic guard-failure test. Name the permitted private test seam and require observation of the warning and absence of action scanning after guard failure.

MINOR

3. SMELL — A2 same-root pin-failure conformance seam

   Issue: The joint pin-failure row is implementable, but the required mechanism is implicit.

   Suggested resolution: State that `scan_worktree_records` may delegate to a private helper using the existing `pub(super) CompatibilityPinOpenerV1`; production supplies `FilesystemCompatibilityPinOpenerV1`, while tests substitute only the pin-open result.

   Disagreement resolved: Soundness is right, because the crate-private opener can be shared by a production-neutral helper, so this is clarification rather than a blocker.

4. SMELL — A1 external public-API evidence

   Issue: The integration test proves that all fifteen type names are publicly re-exported, but internal accessor tests would still compile if a promised accessor were accidentally reduced to `pub(crate)`.

   Suggested resolution: Add a never-called external signature-check function that type-checks the promised public accessors.

Verdict: Changes required before planning — correct the invalid-UTF-8 branch in the A2 mutation audit and AC 20; A1 itself is sound and independently landable.