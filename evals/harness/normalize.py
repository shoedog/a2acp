"""Findings/verdict extraction from a workflow terminal output, for the BLIND
judge.

Per the M3 spec's Judge section, the judge must see ONLY the rubric +
truth.yaml + the normalized findings -- never the raw diff, the reviewer's
tool-call trace, or which cell produced it. Ported IN SPIRIT from
prompts-skills-steering's findings-block normalization (its promptfoo asserts
extracted a findings block out of each provider's raw completion before
handing it to the judge); rebuilt for this repo's review prompts:
`review-correctness.md` / `review-architecture.md` / `review-synth.md` all
explicitly forbid tool preambles and require "Respond with your review as
plain text directly in this reply" + "output the final verdict and STOP", so
in the common case the terminal output IS already a clean findings block --
this module is mostly a safety net against the executor's own bracketed
failure/cancellation markers, plus light whitespace hygiene.
"""
from __future__ import annotations

import re

# bridge-workflow's executor emits these EXACT bracketed markers in place of
# real review prose when a node fails, is canceled, or hits a bad-session
# error -- see crates/bridge-workflow/src/executor.rs, e.g.
#   format!("[node {} failed: {:?}]", node.id.as_str(), e)
#   format!("[node {} canceled]", node.id.as_str())
# Handing one of these to the judge as though it were a review would silently
# grade a harness/agent crash as if it were a (usually clean-looking) review.
_NODE_FAILURE_RE = re.compile(r"^\[node [^\]]+ (?:failed|canceled)\b")


def is_node_failure(raw_output: str) -> bool:
    """True iff `raw_output` (stripped) IS one of the executor's own
    bracketed failure/cancellation markers rather than genuine review prose.

    Callers that want to skip judging a failed call entirely (rather than
    relying on the judge/rubric to score a marker as "no defects found,
    item_pass=false") should check this themselves -- see
    `harness.runner.run_workflow`'s `ok` computation.
    """
    return bool(_NODE_FAILURE_RE.match(raw_output.strip()))


def normalize_findings(raw_output: str) -> str:
    """Return the findings/verdict block to hand the blind judge.

    Currently: strip surrounding whitespace and collapse runs of 3+ blank
    lines down to 2 (cosmetic only). A node-failure marker (see
    `is_node_failure`) is passed through UNCHANGED, not specially rewritten
    -- the judge rubric explicitly instructs the judge to treat a missing/
    unparseable findings block as "found=false everywhere, item_pass=false",
    so the marker text alone is sufficient signal for the judge's own
    fallback rule.

    This is deliberately a thin, conservative pass today. If a live run
    surfaces reviewer preamble/chatter this doesn't already strip (e.g. a
    "Here is my review:" lead-in line the agent adds despite the prompt
    contract), extend this function -- it is the one seam both `runner.py`
    and the standalone `judge.py` CLI both go through, so a fix here reaches
    every call site.
    """
    text = raw_output.strip()
    text = re.sub(r"\n{3,}", "\n\n", text)
    return text
