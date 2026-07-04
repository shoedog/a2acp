"""Tests for `harness.normalize` -- findings extraction + node-failure marker
detection. No model calls."""
from __future__ import annotations

from harness.normalize import is_node_failure, normalize_findings


# --------------------------------------------------------------------------- #
# is_node_failure -- detects bridge-workflow's own bracketed failure markers.
# --------------------------------------------------------------------------- #
def test_is_node_failure_detects_failed_marker():
    assert is_node_failure("[node synth failed: AgentCrashed]")
    assert is_node_failure("[node correctness failed: SomeError { .. }]")


def test_is_node_failure_detects_canceled_marker():
    assert is_node_failure("[node reviewer canceled]")


def test_is_node_failure_tolerates_surrounding_whitespace():
    assert is_node_failure("\n  [node synth failed: boom]\n")


def test_is_node_failure_false_on_real_review_prose():
    assert not is_node_failure(
        "BLOCKER: the lock is held across the .await at line 42.\nVerdict: REJECT."
    )


def test_is_node_failure_false_on_empty_and_prose_mentioning_node():
    assert not is_node_failure("")
    # A real review that merely talks ABOUT a node is not a failure marker --
    # the marker must be the very first thing on the (stripped) line.
    assert not is_node_failure("The node failed to release its lease -- BLOCKER.")


# --------------------------------------------------------------------------- #
# normalize_findings -- whitespace hygiene, failure-marker passthrough.
# --------------------------------------------------------------------------- #
def test_normalize_strips_and_collapses_blank_runs():
    raw = "\n\n  first finding\n\n\n\nsecond finding\n\n\n"
    out = normalize_findings(raw)
    assert out == "first finding\n\nsecond finding"
    assert "\n\n\n" not in out


def test_normalize_preserves_single_blank_line():
    raw = "a\n\nb"
    assert normalize_findings(raw) == "a\n\nb"


def test_normalize_passes_failure_marker_through_unchanged():
    raw = "[node synth failed: AgentCrashed]"
    out = normalize_findings(raw)
    assert out == "[node synth failed: AgentCrashed]"
    # And it is still recognizable as a failure marker after normalization,
    # so the runner's skip check (which runs is_node_failure on the RAW text)
    # and any downstream check agree.
    assert is_node_failure(out)


def test_normalize_empty_input_is_empty():
    assert normalize_findings("") == ""
    assert normalize_findings("   \n\n  ") == ""
