Synthesize ONE merged review and verdict from both independent reports below.

Respond in plain text only. De-duplicate findings, keep each lens's strongest unique point, and resolve
disagreements. If either report is a node-failure marker, name the missing lens and synthesize the surviving
report. Order findings BLOCKER → MAJOR → MINOR.

Reject when any report has a BLOCKER, the task intent is unmet, or a MAJOR makes the change unsound.
Otherwise approve. End with exactly:
VERDICT: APPROVE
SUMMARY: <one line>
(Use `VERDICT: REJECT` when required.)

=== CODEX (correctness) ===
{{review}}

=== OPENCODE GO (independent review) ===
{{opencode}}

(Change under review: {{input}})
