"""CI gate for the M3 eval harness: a deepeval custom NON-LLM `BaseMetric`
artifact-shape gate over a results dir.

Ported IN SPIRIT from prompts-skills-steering's `ci/test_smoke.py`
(`HarnessIntegrityMetric` + `_integrity_failures`): the metric makes NO model
calls of its own -- it only checks that a results dir has the files
`harness/metrics.py` and `harness/report.py` require, with the right shapes.
It is NOT a judgment-quality check (that is the human C2 spot-check's job,
via `spotcheck.yaml`).

Unlike steering's version, this test never runs a live smoke experiment (no
agent turns, no `a2a-bridge` invocation, no network) -- it FABRICATES a tiny
results dir's `calls/`+`judge/` rows directly (see `_fabricate_results_dir`),
then calls `harness.report.render()` on it FOR REAL to produce
`report.md`/`metrics.json`/`spotcheck.{md,yaml}`, then runs the integrity
metric against the resulting directory. This exercises the real
`report.py`/`metrics.py` code path (not a second, hand-maintained shape
description that could drift out of sync with it) while staying entirely
offline.
"""
from __future__ import annotations

import json
from pathlib import Path

import pytest
import yaml
from deepeval import assert_test
from deepeval.metrics import BaseMetric
from deepeval.test_case import LLMTestCase

from harness import report as report_mod

CELLS = ("duo", "codex-solo", "claude-solo")


# --------------------------------------------------------------------------- #
# Fabricated results-dir fixture generator (no agent calls).
# --------------------------------------------------------------------------- #
def _call_row(cell: str, item: str, seeded: bool, truth_path: Path, ok: bool = True) -> dict:
    return {
        "workflow_id": f"code-review-{cell}",
        "cell": cell,
        "item": item,
        "seeded": seeded,
        "returncode": 0 if ok else 1,
        "timed_out": False,
        "elapsed_s": 12.3,
        "ok": ok,
        "stderr_tail": "",
        "out_path": f"calls/{cell}-{item}.md",
        "truth_path": str(truth_path),
    }


def _judge_row(
    cell: str,
    item: str,
    seeded: bool,
    truth_path: Path,
    *,
    item_pass: bool,
    found_ids: list[str] | None = None,
    truth_ids: list[str] | None = None,
    false_findings: int = 0,
    neutral_matched: int = 0,
    judge_error: bool = False,
) -> dict:
    truth_ids = truth_ids if truth_ids is not None else (["d1"] if seeded else [])
    found_ids = found_ids if found_ids is not None else []
    row = {
        "cell": cell,
        "item": item,
        "seeded": seeded,
        "truth_path": str(truth_path),
        "normalized_block": f"[fabricated findings block for {cell}-{item}]",
        "call_ok": True,
        "judge_error": judge_error,
        "item_pass": item_pass,
        "defects": [{"id": tid, "found": tid in found_ids} for tid in truth_ids],
        "false_findings": false_findings,
        "neutral_matched": neutral_matched,
    }
    if judge_error:
        row["judge_error_detail"] = "fabricated judge_error for the integrity gate's own test"
    return row


def _write_fabricated_truth(tmp_path: Path, item: str, seeded: bool) -> Path:
    """A real (tiny) truth.yaml on disk -- `report.render`'s spot-check
    sampling genuinely reads this via `harness.judge.load_truth`, so the
    fixture must be a real file, not just a plausible-looking path string."""
    truth_dir = tmp_path / "tasksets" / "review-seeded-v1" / "items" / item
    truth_dir.mkdir(parents=True, exist_ok=True)
    truth_path = truth_dir / "truth.yaml"
    if seeded:
        truth_path.write_text(
            "seeded: true\n"
            "defects:\n"
            "  - id: d1\n"
            "    description: fabricated defect for the integrity gate's own test\n"
        )
    else:
        truth_path.write_text(
            "seeded: false\n"
            "clean_rationale: fabricated clean item for the integrity gate's own test\n"
            "tempting_non_defects:\n"
            "  - a deliberate clone that looks wasteful but isn't\n"
        )
    return truth_path


def _fabricate_results_dir(tmp_path: Path) -> Path:
    """Write a tiny, internally-consistent fabricated results dir: 2 seeded
    items (s1, s2) + 1 clean item (c1) per cell, one judge_error thrown in
    (duo/s2) to prove `judge_errors` flows through end to end."""
    out_dir = tmp_path / "fabricated-run"
    calls_dir = out_dir / "calls"
    judge_dir = out_dir / "judge"
    calls_dir.mkdir(parents=True)
    judge_dir.mkdir(parents=True)

    items = [("s1", True), ("s2", True), ("c1", False)]
    truth_paths = {item: _write_fabricated_truth(tmp_path, item, seeded) for item, seeded in items}
    for cell in CELLS:
        for item, seeded in items:
            key = f"{cell}-{item}"
            tp = truth_paths[item]
            (calls_dir / f"{key}.md").write_text(f"[fabricated review text for {key}]\n")
            (calls_dir / f"{key}.json").write_text(json.dumps(_call_row(cell, item, seeded, tp), indent=2))

            if cell == "duo" and item == "s2":
                jr = _judge_row(cell, item, seeded, tp, item_pass=False, judge_error=True)
            elif item == "s1":
                jr = _judge_row(cell, item, seeded, tp, item_pass=True, found_ids=["d1"])
            elif item == "s2":
                jr = _judge_row(cell, item, seeded, tp, item_pass=False, found_ids=[])
            else:  # c1, clean
                jr = _judge_row(cell, item, seeded, tp, item_pass=True, false_findings=0)
            (judge_dir / f"{key}.json").write_text(json.dumps(jr, indent=2))

    return out_dir


# --------------------------------------------------------------------------- #
# Integrity-shape check (no model calls).
# --------------------------------------------------------------------------- #
def _load_json(path: Path):
    with open(path) as f:
        return json.load(f)


def _integrity_failures(results_dir: Path, cells: tuple[str, ...], n_items: int) -> list[str]:
    """Every reason `results_dir` is NOT a clean, complete run. Empty list ==
    clean. Makes no model calls -- only reads artifacts already on disk."""
    results_dir = Path(results_dir)
    failures: list[str] = []

    for name in ("report.md", "metrics.json", "spotcheck.yaml"):
        if not (results_dir / name).is_file():
            failures.append(f"missing {name}")

    for cell in cells:
        for sub in ("calls", "judge"):
            paths = sorted((results_dir / sub).glob(f"{cell}-*.json"))
            if len(paths) != n_items:
                failures.append(f"{sub}/: expected {n_items} '{cell}-*.json' file(s), found {len(paths)}")

    # calls/: each row has a bool 'ok' and a numeric 'elapsed_s'.
    for p in sorted((results_dir / "calls").glob("*.json")):
        try:
            rec = _load_json(p)
        except (OSError, json.JSONDecodeError) as e:
            failures.append(f"calls/{p.name}: unparseable ({e})")
            continue
        if not isinstance(rec.get("ok"), bool):
            failures.append(f"calls/{p.name}: 'ok' must be bool, got {rec.get('ok')!r}")
        if not isinstance(rec.get("elapsed_s"), (int, float)):
            failures.append(f"calls/{p.name}: 'elapsed_s' must be numeric")

    # judge/: each row has a bool item_pass and a bool judge_error.
    for p in sorted((results_dir / "judge").glob("*.json")):
        try:
            rec = _load_json(p)
        except (OSError, json.JSONDecodeError) as e:
            failures.append(f"judge/{p.name}: unparseable ({e})")
            continue
        if not isinstance(rec.get("item_pass"), bool):
            failures.append(f"judge/{p.name}: 'item_pass' must be bool, got {rec.get('item_pass')!r}")
        if not isinstance(rec.get("judge_error"), bool):
            failures.append(f"judge/{p.name}: 'judge_error' must be bool, got {rec.get('judge_error')!r}")

    # metrics.json: parses and carries every concept report.py promises.
    metrics_path = results_dir / "metrics.json"
    if metrics_path.is_file():
        try:
            m = _load_json(metrics_path)
        except (OSError, json.JSONDecodeError) as e:
            failures.append(f"metrics.json: unparseable ({e})")
            m = {}
        for key in ("per_cell", "flips", "judge_errors", "taskset_id", "n_items"):
            if key not in m:
                failures.append(f"metrics.json: missing key '{key}'")
        per_cell = m.get("per_cell") or {}
        for cell in cells:
            c = per_cell.get(cell)
            if not isinstance(c, dict) or "pass" not in c or "confusion" not in c:
                failures.append(f"metrics.json: per_cell[{cell!r}] missing 'pass'/'confusion'")
                continue
            if "wilson_ci" not in c["pass"]:
                failures.append(f"metrics.json: per_cell[{cell!r}].pass missing 'wilson_ci'")
            if "defect_recall" not in c["confusion"]:
                failures.append(f"metrics.json: per_cell[{cell!r}].confusion missing 'defect_recall'")

    # report.md: content-level checks, not just presence.
    report_path = results_dir / "report.md"
    if report_path.is_file():
        text = report_path.read_text()
        if "PROVISIONAL" not in text:
            failures.append("report.md: missing the PROVISIONAL banner")
        if "Estimand:" not in text:
            failures.append("report.md: missing the estimand line")
        if f"n={n_items}" not in text:
            failures.append(f"report.md: missing the 'n={n_items}' banner count")

    # spotcheck.yaml: parses to a non-empty 'items' list with the keys a
    # human (or a future check_spotcheck.py) needs to record + join a verdict.
    spotcheck_path = results_dir / "spotcheck.yaml"
    if spotcheck_path.is_file():
        try:
            spot = yaml.safe_load(spotcheck_path.read_text()) or {}
        except yaml.YAMLError as e:
            failures.append(f"spotcheck.yaml: unparseable ({e})")
            spot = {}
        spot_items = spot.get("items") if isinstance(spot, dict) else None
        if not isinstance(spot_items, list) or not spot_items:
            failures.append("spotcheck.yaml: 'items' list missing or empty")
        else:
            for it in spot_items:
                missing = [k for k in ("cell", "item", "agree") if not isinstance(it, dict) or k not in it]
                if missing:
                    failures.append(f"spotcheck.yaml: item missing key(s) {missing}: {it!r}")

    return failures


class HarnessIntegrityMetric(BaseMetric):
    """Binary artifact-integrity gate over one eval results dir. Deliberately
    NOT an LLM-backed metric -- `measure` makes no model calls; it only
    checks that a run wrote every artifact the pipeline promises, with the
    shapes downstream consumers (metrics.py, report.py, the human
    spot-check) require. Threshold 1.0: any single missing/malformed
    artifact fails the whole metric, because a partially-written run is not
    a smaller clean run -- it is a broken one."""

    def __init__(self, results_dir, cells: tuple[str, ...], n_items: int, threshold: float = 1.0):
        self.results_dir = Path(results_dir)
        self.cells = cells
        self.n_items = n_items
        self.threshold = threshold
        self.score = None
        self.reason = None
        self.success = None

    def measure(self, test_case, *args, **kwargs) -> float:
        failures = _integrity_failures(self.results_dir, self.cells, self.n_items)
        self.score = 0.0 if failures else 1.0
        self.reason = "clean run" if not failures else "; ".join(failures)
        self.success = self.score >= self.threshold
        return self.score

    async def a_measure(self, test_case, *args, **kwargs) -> float:
        return self.measure(test_case, *args, **kwargs)

    def is_successful(self) -> bool:
        return bool(self.success)

    @property
    def __name__(self):
        return "HarnessIntegrityMetric"


# --------------------------------------------------------------------------- #
# Tests.
# --------------------------------------------------------------------------- #
def test_fabricated_run_passes_integrity_gate(tmp_path):
    out_dir = _fabricate_results_dir(tmp_path)

    # Exercise the REAL report.py code path against the fabricated calls/judge
    # rows -- report.md/metrics.json/spotcheck.{md,yaml} are genuinely
    # rendered here, not hand-authored a second time.
    report_mod.render(out_dir, taskset_id="fixture-taskset", cells=list(CELLS), n_items=3)

    test_case = LLMTestCase(
        input="fabricated results dir -- artifact integrity check",
        actual_output=str(out_dir),
    )
    assert_test(test_case, [HarnessIntegrityMetric(out_dir, CELLS, n_items=3)])


def test_metric_fails_on_missing_report(tmp_path):
    out_dir = _fabricate_results_dir(tmp_path)
    report_mod.render(out_dir, taskset_id="fixture-taskset", cells=list(CELLS), n_items=3)
    (out_dir / "report.md").unlink()

    test_case = LLMTestCase(
        input="fabricated results dir with report.md deleted -- must fail",
        actual_output=str(out_dir),
    )
    with pytest.raises(AssertionError):
        assert_test(test_case, [HarnessIntegrityMetric(out_dir, CELLS, n_items=3)])


def test_metric_fails_on_missing_judge_rows(tmp_path):
    out_dir = _fabricate_results_dir(tmp_path)
    report_mod.render(out_dir, taskset_id="fixture-taskset", cells=list(CELLS), n_items=3)
    next((out_dir / "judge").glob("duo-*.json")).unlink()

    metric = HarnessIntegrityMetric(out_dir, CELLS, n_items=3)
    score = metric.measure(test_case=None)
    assert score == 0.0
    assert "judge/" in metric.reason
