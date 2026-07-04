"""Render the M3 eval report: metrics.json + report.md (+ spotcheck.yaml for
the human C2 calibration pass).

Ported IN SPIRIT from prompts-skills-steering harness/report.py: a report is
PROVISIONAL until a human confirms a sample of the blind judge's grades --
the judge is instrumentation, not ground truth -- so `report.md` leads with a
provisional banner and `render()` also emits `spotcheck.yaml` (+
`spotcheck.md`, human-readable) with one `agree:` slot per sampled item for
the owner to fill (see the M3 spec's C2 calibration procedure).

Rebuilt around this harness's 3-CELL vocabulary instead of steering's
baseline/treatment arms, and around the fields `calls/*.json` + `judge/*.json`
rows here actually carry (no token/cost accounting, no tiers, no adherence
directives -- see `metrics.py`'s module docstring for what was deliberately
not ported).
"""
from __future__ import annotations

import json
import random

import yaml

from harness.judge import load_truth, render_ground_truth
from harness.metrics import confusion, load_rows, mcnemar_p, pass_rate, paired_flips

_SPOTCHECK_LIMIT = 10  # M3 spec's C2 calibration sample size (steering used 20)

_ESTIMAND = (
    "Estimand: per-cell ABSOLUTE review quality (defect-level catch-rate + "
    "item-level pass/fail confusion) of the bridge's `code-review` workflow "
    "under three fixed configurations -- `duo` (the shipping shape, graded on "
    "the SYNTH output), `codex-solo` and `claude-solo` (single reviewer, "
    "graded RAW -- no synth pass). This is NOT a controlled ablation: cells "
    "differ in agent composition AND the presence/absence of a synth pass "
    "simultaneously, so cross-cell deltas below are a descriptive "
    "paired-flips display, never a pass/fail criterion on their own."
)

# Cross-cell comparisons the report renders as a paired-flips display -- the
# shipping `duo` shape against each single-reviewer ablation. `codex-solo`
# vs `claude-solo` is not paired here (neither is a baseline for the other);
# add it if a future report needs it.
_PAIRS = (("duo", "codex-solo"), ("duo", "claude-solo"))


def summarize(out_dir, cells: list[str]) -> dict:
    calls, judges = load_rows(out_dir)
    per_cell = {}
    for cell in cells:
        per_cell[cell] = {
            "pass": pass_rate(judges, cell),
            "confusion": confusion(judges, cell),
        }

    flips = []
    for a, b in _PAIRS:
        if a in cells and b in cells:
            fp = paired_flips(judges, a, b)
            fp["mcnemar_p"] = mcnemar_p(fp["only_a"], fp["only_b"])
            flips.append(fp)

    judge_errors = sum(per_cell[c]["pass"]["judge_errors"] for c in cells)
    return {
        "calls": calls,
        "judges": judges,
        "per_cell": per_cell,
        "flips": flips,
        "judge_errors": judge_errors,
    }


def _pass_line(pr: dict) -> str:
    lo, hi = pr["wilson_ci"]
    return f"{pr['k']}/{pr['n']} = {pr['rate']:.3f}  (95% CI {lo:.3f}-{hi:.3f})"


def _floor_flag(s: dict, cells: list[str], threshold: float = 0.15) -> bool:
    """True iff EVERY cell's item-pass rate floors below `threshold` -- the
    composite pass rate is uninformative in that case and the report should
    lead with defect recall instead (steering's `composite_floored`,
    generalized from 2 arms to N cells: ALL must floor, not just one)."""
    return all(s["per_cell"][c]["pass"]["rate"] < threshold for c in cells) if cells else False


def render_report_md(s: dict, cells: list[str], taskset_id: str, n_items: int) -> str:
    L: list[str] = []
    L.append("# M3 review-quality eval report -- PROVISIONAL / n=" + str(n_items))
    L.append("")
    L.append(
        f"> PROVISIONAL -- pending the owner's C2 spot-check. Fill `spotcheck.yaml`'s "
        f"`agree:` fields, then re-run the eyeball pass described in evals/README.md."
    )
    L.append("")
    L.append(f"- taskset: `{taskset_id}`  n={n_items}  cells: {', '.join(cells)}")
    L.append("")
    L.append(_ESTIMAND)
    L.append("")

    if _floor_flag(s, cells):
        L.append("## ⚠ COMPOSITE FLOORED -- composite pass rate INCONCLUSIVE")
        L.append(
            "Every cell floored below 0.15 item-pass. Lead with defect-level "
            "recall and verdict confusion below, not the composite pass rate."
        )
        L.append("")

    L.append("## Per-cell results")
    L.append("")
    L.append("| cell | n | item-pass (95% CI) | defect recall | false findings (clean) | judge errors |")
    L.append("|---|---|---|---|---|---|")
    for cell in cells:
        pr = s["per_cell"][cell]["pass"]
        c = s["per_cell"][cell]["confusion"]
        dr = c["defect_recall"]
        L.append(
            f"| {cell} | {pr['n']} | {_pass_line(pr)} | "
            f"{dr['found']}/{dr['total']} = {dr['rate']:.3f} | "
            f"{c['false_findings_clean_total']} (of {c['false_findings_total']} total) | "
            f"{pr['judge_errors']} |"
        )
    L.append("")

    L.append("## Confusion matrix (TP/FP/TN/FN, base rate)")
    L.append("")
    L.append("| cell | TP | FP | TN | FN | base rate | neutral matched |")
    L.append("|---|---|---|---|---|---|---|")
    for cell in cells:
        c = s["per_cell"][cell]["confusion"]
        L.append(
            f"| {cell} | {c['tp']} | {c['fp']} | {c['tn']} | {c['fn']} | "
            f"{c['base_rate']:.3f} | {c['neutral_matched_total']} |"
        )
    L.append("")

    L.append("## Paired flip table (descriptive display, NOT a pass/fail criterion)")
    L.append("")
    if not s["flips"]:
        L.append("_No comparable cell pair present in this run._")
    for fp in s["flips"]:
        L.append(
            f"- **{fp['cell_a']} vs {fp['cell_b']}**: both_pass={fp['both_pass']} "
            f"both_fail={fp['both_fail']} only_{fp['cell_a']}={fp['only_a']} "
            f"only_{fp['cell_b']}={fp['only_b']}  "
            f"McNemar exact p={fp['mcnemar_p']:.3f}"
        )
    L.append("")
    L.append(
        "McNemar's p is REPORTED above for reference only -- per the M3 spec it is "
        "demoted to a paired-flips display and is never treated as a pass/fail gate, "
        "and no winner language is used here unless effects are huge and the paired "
        "flips are obvious."
    )
    L.append("")

    L.append("## Judge errors")
    L.append("")
    L.append(f"- total judge_error rows across all cells: {s['judge_errors']}")
    L.append(
        "  (excluded from every rate/confusion figure above -- a judge_error is a "
        "harness/agent failure, never silently folded into a result.)"
    )
    L.append("")
    L.append("---")
    L.append(
        "_Not aggregated across tasksets or repo states. See `run_manifest.json` in "
        "this results dir for the bridge git SHA, cell configs, and taskset id this "
        "report was rendered against._"
    )
    L.append("")
    return "\n".join(L)


# --------------------------------------------------------------------------- #
# Spot-check sample (C2 human calibration).
# --------------------------------------------------------------------------- #
def _sample_spotcheck(judges: list[dict], limit: int = _SPOTCHECK_LIMIT, seed: int = 0) -> list[dict]:
    """Up to `limit` judged items for the human C2 calibration pass (M3 spec:
    "a random 10-item judge-decision sample").

    Stratified round-robin across cells so a small sample can't crowd one
    cell out entirely -- the same failure mode steering's own spot-check
    sampling was fixed to avoid (naive `graded[:N]` off an alphabetically
    sorted glob put 20/20 sampled rows in one arm on a real run). Uses a
    FIXED seed so re-running `render()` over the same judge/*.json rows
    reproduces the same sample rather than a new one every time -- "random"
    per the spec, reproducible in practice.
    """
    graded = [r for r in judges if not r.get("judge_error")]
    by_cell: dict[str, list[dict]] = {}
    for r in graded:
        by_cell.setdefault(r.get("cell"), []).append(r)

    rng = random.Random(seed)
    for rows in by_cell.values():
        rng.shuffle(rows)
    cells_order = sorted((c for c in by_cell if c is not None), key=str)

    out: list[dict] = []
    i = 0
    while len(out) < limit and any(i < len(by_cell[c]) for c in cells_order):
        for c in cells_order:
            if len(out) >= limit:
                break
            if i < len(by_cell[c]):
                out.append(by_cell[c][i])
        i += 1
    return out[:limit]


def render_spotcheck(judges: list[dict]) -> tuple[str, list[dict]]:
    md = ["# Judge spot-check (human review)", ""]
    yaml_rows = []
    for r in _sample_spotcheck(judges):
        truth_path = r.get("truth_path")
        truth = load_truth(truth_path) if truth_path else {}
        gt = render_ground_truth(truth)
        found = [d["id"] for d in (r.get("defects") or []) if d.get("found")]
        md.append(f"## {r.get('cell')} -- {r.get('item')} (seeded={r.get('seeded')})")
        md.append("")
        md.append("**Normalized findings block:**")
        md.append("```")
        md.append((r.get("normalized_block") or "").rstrip())
        md.append("```")
        md.append("**Ground truth:**")
        md.append("```")
        md.append(gt)
        md.append("```")
        md.append(
            f"**Judge:** item_pass={r.get('item_pass')}  found={found}  "
            f"false_findings={r.get('false_findings')}  "
            f"neutral_matched={r.get('neutral_matched')}"
        )
        md.append("")
        yaml_rows.append(
            {
                "cell": r.get("cell"),
                "item": r.get("item"),
                "seeded": r.get("seeded"),
                "found": found,
                "false_findings": r.get("false_findings"),
                "item_pass": r.get("item_pass"),
                "agree": None,
            }
        )
    return "\n".join(md), yaml_rows


# --------------------------------------------------------------------------- #
# Top-level entry point.
# --------------------------------------------------------------------------- #
def render(out_dir, *, taskset_id: str, cells: list[str], n_items: int, note: str | None = None) -> dict:
    """Compute metrics and write report.md, metrics.json, spotcheck.md,
    spotcheck.yaml into `out_dir`. Returns the summary dict (so
    `runner.py` can read `judge_errors` for its own exit code)."""
    out_dir = str(out_dir)
    s = summarize(out_dir, cells)

    report_md = render_report_md(s, cells, taskset_id, n_items)
    if note:
        report_md = f"> NOTE -- {note}\n\n" + report_md
    with open(f"{out_dir}/report.md", "w") as f:
        f.write(report_md)

    metrics_json = {k: v for k, v in s.items() if k not in ("calls", "judges")}
    metrics_json["taskset_id"] = taskset_id
    metrics_json["n_items"] = n_items
    with open(f"{out_dir}/metrics.json", "w") as f:
        json.dump(metrics_json, f, ensure_ascii=False, indent=2)

    spot_md, spot_rows = render_spotcheck(s["judges"])
    with open(f"{out_dir}/spotcheck.md", "w") as f:
        f.write(spot_md)
    with open(f"{out_dir}/spotcheck.yaml", "w") as f:
        yaml.safe_dump({"items": spot_rows}, f, sort_keys=False, allow_unicode=True)

    return s


def main(argv=None) -> int:
    """Standalone CLI: (re)render report.md/metrics.json/spotcheck.{md,yaml}
    from an EXISTING results dir's calls/+judge/ rows -- no agent calls, no
    re-run of the experiment. Useful whenever this module's rendering logic
    changes and already-run results need their report regenerated to match."""
    import argparse

    p = argparse.ArgumentParser(prog="harness.report")
    p.add_argument("results_dir")
    p.add_argument("--taskset-id", required=True)
    p.add_argument("--cells", default="duo,codex-solo,claude-solo")
    p.add_argument("--n-items", type=int, required=True)
    args = p.parse_args(argv)

    cells = [c.strip() for c in args.cells.split(",") if c.strip()]
    render(args.results_dir, taskset_id=args.taskset_id, cells=cells, n_items=args.n_items)
    print(f"OK: rendered {args.results_dir}/{{report.md,metrics.json,spotcheck.md,spotcheck.yaml}}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
