#!/usr/bin/env python3
"""Check per-crate line coverage from one cargo-llvm-cov JSON report."""

from __future__ import annotations

import json
import sys
from argparse import ArgumentParser
from pathlib import PurePosixPath


# Floors are the integer floor of each crate's measured line coverage from the single
# instrumented workspace run (all tests together), PR #218 CI, 2026-10-24.
THRESHOLDS = {
    "bridge-core": 92.0,  # measured 92.996%
    "bridge-acp": 95.0,  # measured 95.558%
    "bridge-api": 96.0,  # measured 96.318%
    "bridge-workflow": 91.0,  # measured 91.279%
    "bridge-coordinator": 92.0,  # measured 92.427%
    "bridge-mcp": 80.0,  # measured 80.995%
}


def source_crate(filename: str) -> str | None:
    """Return the workspace crate for a source path in an llvm-cov report."""
    parts = PurePosixPath(filename.replace("\\", "/")).parts
    for index, part in enumerate(parts[:-2]):
        if part == "crates" and parts[index + 2] == "src":
            return parts[index + 1]
    return None


def main() -> int:
    parser = ArgumentParser(description=__doc__)
    parser.add_argument("report", help="cargo llvm-cov JSON report")
    parser.add_argument(
        "--crate",
        dest="crates",
        action="append",
        choices=THRESHOLDS,
        help="check one crate (repeatable); defaults to every gated crate",
    )
    args = parser.parse_args()

    with open(args.report, encoding="utf-8") as report_file:
        report = json.load(report_file)

    selected = args.crates or list(THRESHOLDS)
    totals = {crate: [0, 0] for crate in selected}
    for datum in report.get("data", []):
        for file in datum.get("files", []):
            crate = source_crate(file["filename"])
            if crate not in totals:
                continue
            lines = file["summary"]["lines"]
            totals[crate][0] += lines["covered"]
            totals[crate][1] += lines["count"]

    failed = False
    for crate in selected:
        threshold = THRESHOLDS[crate]
        covered, count = totals[crate]
        if count == 0:
            print(f"{crate}: no source-file coverage data", file=sys.stderr)
            failed = True
            continue
        percent = covered * 100 / count
        print(
            f"{crate}: {percent:.2f}% "
            f"({covered}/{count} lines; threshold {threshold:.2f}%)"
        )
        if percent < threshold:
            print(f"{crate}: below required line coverage", file=sys.stderr)
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
