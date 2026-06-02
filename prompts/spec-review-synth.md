Synthesize ONE merged spec review from the two independent reviews below.

OUTPUT CONTRACT — follow exactly:
- Respond with the merged review as plain text ONLY. Do NOT use any tools or read/write files. When complete, STOP.

HOW TO MERGE:
- De-duplicate; keep each lens's strongest unique points (Rigor = completeness / ambiguity / verifiability; Soundness = design / decomposition fit).
- Resolve disagreements explicitly — which lens is right and why, in one line.
- If one lens reported an error marker instead of a review (a node failed), note the missing lens and synthesize from the one that succeeded.

OUTPUT FORMAT:
A single prioritized list, **BLOCKER → MAJOR → MINOR**, each with section, issue, and suggested resolution. End with a one-line verdict: ready to plan, or the specific changes required first.

=== RIGOR (completeness / ambiguity lens) ===
{{rigor}}

=== SOUNDNESS (design lens) ===
{{soundness}}

(Original spec under review, for reference: {{input}})
