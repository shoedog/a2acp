Synthesize ONE merged review from the two independent reviews below.

OUTPUT CONTRACT — follow exactly:
- Respond with the merged review as plain text ONLY, directly in this reply.
- Do NOT use any tools. Do NOT read or write files. Do NOT run commands or searches. Everything you need is below.
- When the merged review is complete, STOP.

HOW TO MERGE:
- De-duplicate overlapping findings; keep each reviewer's strongest unique points (Codex weighs blockers/correctness; Claude weighs architecture).
- Resolve any disagreement explicitly — say which reviewer is right and why, in one line.
- If a reviewer reported an error marker instead of a review (a node failed), note the lens is missing and synthesize from the lens that succeeded.

OUTPUT FORMAT:
A single prioritized list, **BLOCKER → MAJOR → MINOR**, each with location, the issue, and the fix. Then a one-line overall verdict (e.g. "ship after fixing the 2 BLOCKERs").

=== CORRECTNESS LENS (default: codex) ===
{{correctness}}

=== ARCHITECTURE LENS (default: claude) ===
{{architecture}}

(Original artifact under review, for reference: {{input}})
