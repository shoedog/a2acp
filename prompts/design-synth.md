Synthesize ONE design from the two INDEPENDENT designs below. Each was produced CLEAN-ROOM — neither architect saw the other's work — so agreement between them is strong signal.

READ-ONLY + BOUNDED CONTRACT — follow exactly:
- Your job is to MERGE, not to re-design from scratch. You MAY use READ-ONLY tools (read/grep/`git diff`) ONLY to adjudicate a disagreement — verify which architect's claim about the actual code is correct. Do NOT modify anything.
- When the merged design is done, output it and STOP.

HOW TO MERGE:
- **Convergent spine:** what BOTH architects chose independently is high-confidence — state it as the design's spine.
- **Divergences:** for each, pick the stronger option with a one-line why (verify against the code if it's a factual claim); integrate complementary pieces from each rather than discarding one wholesale.
- **Unresolved tradeoffs:** where the choice is a genuine judgment call the owner should make, surface it explicitly — do NOT silently pick one and bury the other.
- If one architect produced an error marker instead of a design (its node failed), synthesize from the one that succeeded and note the missing lens.

OUTPUT — one coherent design:
- Approach + component/file boundaries; key interfaces/types; the flow; decisions + rationale; risks; the smallest shippable slices + build order.
- A final **DECISIONS FOR THE OWNER** list (the unresolved tradeoffs, each with the options + a recommendation).
- End with a one-line readiness verdict (e.g. "ready to plan after deciding the 2 open questions").

=== EXECUTABILITY LENS (default: codex) ===
{{executability}}

=== STRUCTURE LENS (default: claude) ===
{{structure}}

(Problem statement, for reference: {{input}})
