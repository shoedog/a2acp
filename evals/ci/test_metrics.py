"""Metrics tests -- synthetic rows only, no model calls, no `a2a-bridge`
invocations. Known-value expectations PORTED from
prompts-skills-steering's `harness/tests/test_metrics.py` (wilson_ci,
mcnemar_p); `confusion`/`paired_flips` tests rebuilt against this harness's
`cell`/`id`/`item_pass` row shape (see `harness/metrics.py`'s module
docstring for the exact deltas from steering's version).
"""
from __future__ import annotations

import pytest

from harness.metrics import (
    IntegrityError,
    _load_json_glob,
    confusion,
    mcnemar_p,
    pass_rate,
    paired_flips,
    wilson_ci,
)


def _judge_row(cell="duo", item="t0", **kw):
    base = {
        "cell": cell,
        "item": item,
        "seeded": kw.pop("seeded", True),
        "item_pass": kw.pop("item_pass", False),
        "judge_error": kw.pop("judge_error", False),
        "defects": kw.pop("defects", []),
        "false_findings": kw.pop("false_findings", 0),
        "neutral_matched": kw.pop("neutral_matched", 0),
    }
    base.update(kw)
    return base


# --------------------------------------------------------------------------- #
# Wilson CI -- ported verbatim, same known values as steering.
# --------------------------------------------------------------------------- #
def test_wilson_ci_known_case():
    lo, hi = wilson_ci(7, 10)
    assert lo == pytest.approx(0.397, abs=0.005)
    assert hi == pytest.approx(0.892, abs=0.005)


def test_wilson_ci_zero_n():
    assert wilson_ci(0, 0) == (0.0, 0.0)


# --------------------------------------------------------------------------- #
# pass_rate.
# --------------------------------------------------------------------------- #
def test_pass_rate_excludes_judge_errors_but_counts_them():
    rows = [
        _judge_row(item_pass=True, judge_error=False),
        _judge_row(item_pass=False, judge_error=False),
        _judge_row(item_pass=True, judge_error=True),  # excluded from k/n
        _judge_row(cell="codex-solo", item_pass=True),  # other cell ignored
    ]
    pr = pass_rate(rows, "duo")
    assert pr["n"] == 2
    assert pr["k"] == 1
    assert pr["rate"] == pytest.approx(0.5)
    assert pr["judge_errors"] == 1
    assert pr["call_failures"] == 0


def test_pass_rate_excludes_call_failed_rows_and_counts_them():
    # MAJOR-1 skip path: a reviewer-call failure produces a call_failed row
    # (judge never invoked). It must be counted as NEITHER pass nor fail, and
    # surfaced separately as `call_failures`.
    rows = [
        _judge_row(item="t1", item_pass=True),
        _judge_row(item="t2", item_pass=False),
        _judge_row(item="t3", item_pass=None, call_failed=True),  # excluded
    ]
    pr = pass_rate(rows, "duo")
    assert pr["n"] == 2  # only the two real verdicts
    assert pr["k"] == 1
    assert pr["rate"] == pytest.approx(0.5)
    assert pr["call_failures"] == 1
    assert pr["judge_errors"] == 0


# --------------------------------------------------------------------------- #
# confusion -- item-level TP/FP/TN/FN + defect-level recall (adapted: no
# verdict_flagged tag, `flagged` derived from found/false_findings).
# --------------------------------------------------------------------------- #
def test_confusion_hand_built_six_rows():
    rows = [
        _judge_row(seeded=True, item_pass=True,
                   defects=[{"id": "a", "found": True}, {"id": "b", "found": True}]),  # flagged (found) -> TP, 2/2
        _judge_row(seeded=True, item_pass=True,
                   defects=[{"id": "c", "found": True}]),  # TP, 1/1
        _judge_row(seeded=True, item_pass=False,
                   defects=[{"id": "d", "found": False}]),  # not flagged -> FN, 0/1
        _judge_row(seeded=False, item_pass=False, false_findings=1),  # flagged (false_findings>0) -> FP
        _judge_row(seeded=False, item_pass=True),  # not flagged -> TN
        _judge_row(seeded=False, item_pass=True),  # TN
    ]
    c = confusion(rows, "duo")
    assert (c["tp"], c["fp"], c["tn"], c["fn"]) == (2, 1, 2, 1)
    assert c["n"] == 6
    assert c["base_rate"] == pytest.approx(0.5)  # (TP+FN)/n = 3/6
    assert c["defect_recall"]["found"] == 3
    assert c["defect_recall"]["total"] == 4
    assert c["defect_recall"]["rate"] == pytest.approx(0.75)
    assert c["false_findings_total"] == 1
    assert c["false_findings_clean_total"] == 1  # the one false finding was on a clean row
    assert c["neutral_matched_total"] == 0


def test_confusion_sums_neutral_matched_separately_from_false_findings():
    rows = [
        _judge_row(seeded=True, item_pass=True, defects=[{"id": "a", "found": True}],
                   false_findings=0, neutral_matched=2),
        _judge_row(seeded=True, item_pass=True, defects=[{"id": "b", "found": True}],
                   false_findings=1, neutral_matched=1),
        _judge_row(seeded=False, item_pass=True),  # no neutral key -> 0
    ]
    c = confusion(rows, "duo")
    assert c["neutral_matched_total"] == 3
    assert c["false_findings_total"] == 1
    assert c["false_findings_clean_total"] == 0  # the false finding was on a SEEDED row, not clean


def test_confusion_defect_recall_anchored_to_truth_ids_present_in_the_row():
    # harness.judge._parse_and_validate already refuses a judge response that
    # omits a truth id or invents one not in truth, so by the time a row
    # reaches confusion(), its 'defects' list IS exactly the truth id set --
    # this test documents that invariant's consequence for recall accounting.
    rows = [
        _judge_row(seeded=True, item_pass=False,
                   defects=[{"id": "d1", "found": True}, {"id": "d2", "found": False}]),
    ]
    c = confusion(rows, "duo")
    dr = c["defect_recall"]
    assert dr["total"] == 2
    assert dr["found"] == 1
    assert dr["rate"] == pytest.approx(0.5)


def test_confusion_excludes_judge_error_rows():
    rows = [
        _judge_row(seeded=True, item_pass=True, defects=[{"id": "a", "found": True}]),
        _judge_row(seeded=True, item_pass=True, defects=[{"id": "b", "found": True}], judge_error=True),
    ]
    c = confusion(rows, "duo")
    assert c["n"] == 1


def test_confusion_excludes_call_failed_rows_from_all_figures():
    # A call_failed row on a CLEAN item is the exact collision MAJOR-1 fixes:
    # if it were scored it could become a clean TN and inflate specificity, or
    # (seeded) drag recall to 0/1. It must contribute to nothing.
    rows = [
        _judge_row(seeded=True, item_pass=True, defects=[{"id": "a", "found": True}]),
        # a seeded call_failed row: no defects, would otherwise read as FN + 0/1 recall
        _judge_row(seeded=True, item_pass=None, defects=[], call_failed=True),
        # a clean call_failed row: would otherwise read as a TN
        _judge_row(seeded=False, item_pass=None, call_failed=True),
    ]
    c = confusion(rows, "duo")
    assert c["n"] == 1  # only the one real verdict
    assert (c["tp"], c["fp"], c["tn"], c["fn"]) == (1, 0, 0, 0)
    assert c["defect_recall"]["total"] == 1
    assert c["defect_recall"]["found"] == 1


def test_paired_flips_excludes_pair_when_either_side_call_failed():
    rows = [
        _judge_row(cell="duo", item="t1", item_pass=True),
        _judge_row(cell="codex-solo", item="t1", item_pass=True),  # both_pass
        _judge_row(cell="duo", item="t2", item_pass=True),
        _judge_row(cell="codex-solo", item="t2", item_pass=None, call_failed=True),  # excluded pair
    ]
    fp = paired_flips(rows, "duo", "codex-solo")
    assert fp["both_pass"] == 1
    assert fp["both_fail"] == 0
    assert fp["only_a"] == 0
    assert fp["only_b"] == 0


def test_load_json_glob_collects_named_integrity_errors(tmp_path):
    (tmp_path / "good.json").write_text('{"ok": true}')
    (tmp_path / "truncated.json").write_text('{"ok": true')  # never closed
    (tmp_path / "garbage.json").write_text("not json at all")
    with pytest.raises(IntegrityError) as exc:
        _load_json_glob(str(tmp_path / "*.json"))
    msg = str(exc.value)
    assert "truncated.json" in msg
    assert "garbage.json" in msg
    assert "good.json" not in msg


# --------------------------------------------------------------------------- #
# paired_flips -- generalized to two named cells.
# --------------------------------------------------------------------------- #
def test_paired_flips_joins_on_item_id():
    rows = [
        _judge_row(cell="duo", item="t1", item_pass=True),
        _judge_row(cell="codex-solo", item="t1", item_pass=True),  # both_pass
        _judge_row(cell="duo", item="t2", item_pass=True),
        _judge_row(cell="codex-solo", item="t2", item_pass=False),  # only_a (duo)
        _judge_row(cell="duo", item="t3", item_pass=False),
        _judge_row(cell="codex-solo", item="t3", item_pass=True),  # only_b (codex-solo)
        _judge_row(cell="duo", item="t4", item_pass=False),
        _judge_row(cell="codex-solo", item="t4", item_pass=False),  # both_fail
        _judge_row(cell="duo", item="t5", item_pass=True),
        _judge_row(cell="codex-solo", item="t5", item_pass=True, judge_error=True),  # excluded
    ]
    fp = paired_flips(rows, "duo", "codex-solo")
    assert fp["both_pass"] == 1
    assert fp["both_fail"] == 1
    assert fp["only_a"] == 1
    assert fp["only_b"] == 1
    assert fp["cell_a"] == "duo"
    assert fp["cell_b"] == "codex-solo"


# --------------------------------------------------------------------------- #
# McNemar exact p-value -- ported verbatim, same known values as steering.
# --------------------------------------------------------------------------- #
def test_mcnemar_p_zero_discordant_pairs_is_one():
    assert mcnemar_p(0, 0) == 1.0


def test_mcnemar_p_one_vs_one_is_one():
    assert mcnemar_p(1, 1) == pytest.approx(1.0)


def test_mcnemar_p_symmetric_in_its_arguments():
    assert mcnemar_p(2, 8) == pytest.approx(mcnemar_p(8, 2))


def test_mcnemar_p_highly_asymmetric_is_significant():
    p = mcnemar_p(0, 16)
    assert p < 0.05
    assert p == pytest.approx(2 * (0.5**16))


def test_mcnemar_p_known_hand_computed_value():
    # n=10, b=2, c=8: P(X<=2|n=10,p=.5) = (C(10,0)+C(10,1)+C(10,2))/1024
    #                                    = (1+10+45)/1024 = 56/1024 = 0.0546875
    # p = 2 * 0.0546875 = 0.109375
    assert mcnemar_p(2, 8) == pytest.approx(0.109375)


def test_mcnemar_p_never_exceeds_one():
    for b, c in ((1, 1), (2, 2), (3, 3), (0, 0), (5, 4)):
        assert mcnemar_p(b, c) <= 1.0
