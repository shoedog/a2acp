Synthesize ONE weighted panel recommendation from the independent member analyses below.

OUTPUT CONTRACT — follow exactly:
- Respond with the panel as plain text ONLY, directly in this reply.
- Do NOT use any tools. Do NOT read or write files. Do NOT run commands or searches. Everything you need is below.
- When the panel is complete, STOP.

HOW TO BUILD THE PANEL (in THIS order):
1. FIRST, output a section headed `## Per-source usage`, then reproduce the ENTIRE "PER-MEMBER USAGE" markdown table below EXACTLY as given — copy it verbatim, byte-for-byte: do NOT reformat, reorder rows, round, drop the header, or change any number or `n/a` cell. This exact table must appear in your output unchanged.
2. THEN, for EACH member, a compact block: **Pros / Cons / Usage / Benefit / Risk**. Take each member's Usage from that same table (do not invent numbers).
3. THEN apply the operator-configured WEIGHTS below to reach a weighted recommendation. State the weights you applied and name the winner + why in one line.
4. If a member reported an error marker instead of an analysis (a node failed), note the lens is missing, synthesize from the survivors, and show its usage as n/a (its row in the table above already shows n/a).

=== OPERATOR WEIGHTS ===
{{workflow.weights}}

=== PER-MEMBER USAGE (real, captured) ===
{{workflow.costs}}

=== MEMBER A ===
{{member_a}}

=== MEMBER B ===
{{member_b}}

(Original input, for reference: {{input}})
