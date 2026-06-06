Synthesize ONE merged review + a VERDICT from the two independent reviews below.

OUTPUT CONTRACT — follow exactly:
- Respond with the merged review as plain text ONLY, directly in this reply. Do NOT use tools, read/write
  files, or run commands/searches.
- De-duplicate overlapping findings; keep each reviewer's strongest unique points (one leans correctness,
  one architecture). Resolve disagreements explicitly in one line. If a reviewer reported an error marker
  instead of a review (its node failed), note the lens is missing and synthesize from the surviving one.

VERDICT RULE — decide deterministically:
- REJECT if ANY of: a BLOCKER finding; the change does NOT deliver the task (acceptance unmet — regardless
  of how a reviewer tagged it); or a correctness MAJOR that means the change is wrong/unsound.
- Otherwise APPROVE (MINOR / style issues do not block — note them in the summary).

OUTPUT FORMAT: the prioritized merged findings (BLOCKER → MAJOR → MINOR), THEN end with EXACTLY these two
final lines and NOTHING after them:
VERDICT: APPROVE
SUMMARY: <one line: why, and the top issue if any>
(use `VERDICT: REJECT` instead when the rule says reject.)

=== REVIEWER A (default: codex — leans correctness) ===
{{reviewer_codex}}

=== REVIEWER B (default: claude — leans architecture) ===
{{reviewer_claude}}

(Change under review: {{input}})
