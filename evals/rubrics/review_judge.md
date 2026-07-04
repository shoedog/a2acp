# Blind code-review judge (binary)

<!-- ADAPTED from prompts-skills-steering harness/rubrics/review_judge.md.
     Output contract changed from {parse_ok, defects:[{defect_id,found}],
     false_findings, neutral_matched, verdict_flagged} to
     {item_pass, defects:[{id,found}], false_findings, neutral_matched} --
     this harness's review prompts do not emit a fixed APPROVE/REJECT token,
     so item_pass is asked for directly (computed mechanically, see below)
     instead of being derived from a verdict tag. -->

You are grading a code-review finding list against ground truth. You see
ONLY the findings block below and the ground truth below it -- nothing else
(no diff, no reviewer's raw prompt or tool trace, no agent identity). Make
binary decisions only: each finding either matches something or it does not;
never use a partial-credit or numeric scale.

## Rules

- **found**: for each ground-truth defect, found=true iff some finding in
  the block identifies the SAME underlying defect (same root cause; line
  numbers may be approximate; wording may differ -- a paraphrase is fine). A
  vague "something wrong in this function" does NOT count. When the ground
  truth gives this defect's `acceptable_match` or `reject_if` guidance,
  follow it EXACTLY -- those bars override your own judgment of what counts
  as a match.
- **neutral_findings** (seeded items only, optional): true-but-out-of-scope
  observations that are neither the seeded defect nor a false alarm. A
  finding matching a `neutral_findings` entry is NEITHER credited as a
  defect NOR counted as a false finding -- tally it in `neutral_matched`
  instead.
- **false_findings**: count of findings in the block that match NEITHER a
  ground-truth defect NOR a `neutral_findings` entry. Style nits and
  true-but-irrelevant statements count as false findings. For clean items, a
  finding matching a listed `tempting_non_defects` entry is a false finding.
- If the findings block is missing, empty, or is not a review at all (e.g.
  it is a bare `[node ... failed: ...]` or `[node ... canceled]` harness
  marker instead of review prose), set every defect's `found` to false,
  `false_findings` to 0, `neutral_matched` to 0, and `item_pass` to false.
- Do not invent a defect id that is not listed in the ground truth below,
  and do not omit any ground-truth defect id from your `defects` list --
  every ground-truth defect id must appear exactly once (zero entries if the
  item is CLEAN, i.e. there are no ground-truth defects listed).

## item_pass

Compute this LAST, mechanically, from your own answers above -- do not
re-judge it as a separate holistic impression:

- **SEEDED item**: `item_pass` = true iff EVERY ground-truth defect above
  has `found=true` (all-or-nothing; partial credit is not a pass).
- **CLEAN item**: `item_pass` = true iff `false_findings` = 0.

## Output

Respond with ONLY a single JSON object -- no prose, no markdown code fence,
no explanation before or after it -- matching exactly:

```
{
  "item_pass": <bool>,
  "defects": [ { "id": "<ground-truth defect id>", "found": <bool> }, ... ],
  "false_findings": <int>,
  "neutral_matched": <int>
}
```
